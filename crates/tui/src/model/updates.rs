use super::*;

impl Model {

    /// Fold one event into the state.
    pub(super) fn reduce(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::Wire(_) | UiEvent::Component(_) => unreachable!("intake is dispatched before reduction"),
            UiEvent::Agent(ev) => self.apply_agent(ev),
            UiEvent::Approval(req) => {
                if let Some(row) = self.agents.get_mut(&req.session) {
                    row.info.state = AgentState::Approval {
                        tool: req.tool.clone(),
                    };
                    row.tool = Some(format!(
                        "{} {}",
                        req.tool,
                        describe(&req.tool, &req.args).join(" ")
                    ));
                }
                if !self.approvals.iter().any(|pending| pending.id == req.id)
                    && (self.session == Some(req.session) || self.agents.contains_key(&req.session))
                {
                    self.approvals.push_back(req);
                }
            }
            UiEvent::Resolved(r) => {
                if let Some(req) = self.approvals.iter().find(|a| a.id == r.id) {
                    let session = req.session;
                    self.approval_done(session);
                }
                self.approvals.retain(|a| a.id != r.id);
                self.double.remove(&r.id.0.to_string());
            }
            UiEvent::Ask(req) => {
                if !req.questions.is_empty()
                    && !self.asks.iter().any(|pending| pending.req.id == req.id)
                    && (self.session == Some(req.session) || self.agents.contains_key(&req.session))
                {
                    self.asks.push_back(Ask::new(req));
                }
            }
            UiEvent::AskResolved(r) => {
                self.asks.retain(|a| a.req.id != r.id);
                self.double.remove(&r.id.0.to_string());
            }
            UiEvent::DoublePredicted(p) => {
                self.double.insert(p.question_id.clone(), p);
            }
            UiEvent::DoubleAnswered(a) => {
                self.double.remove(&a.question_id);
                if a.session.is_none_or(|s| {
                    self.session == Some(s) || self.agents.contains_key(&s)
                }) {
                    self.entries.push(Entry::Info(double::chip::answered(&a)));
                }
            }
            UiEvent::Sessions(list) => {
                if list.is_empty() {
                    self.entries.push(Entry::Info("no sessions".into()));
                } else {
                    let items = session_rows(list);
                    // A refresh of an open menu keeps the highlight where it was.
                    let cursor = match &self.menu {
                        Some(Menu::Sessions { cursor, .. }) => *cursor,
                        _ => items
                            .iter()
                            .position(|r| r.item().is_some_and(|(s, _)| Some(s.id) == self.session))
                            .unwrap_or_else(|| grouped_first(&items)),
                    };
                    self.menu = Some(Menu::Sessions { items, cursor });
                }
            }
            UiEvent::Search(generation, reply) => self.search_reply(generation, reply),
            UiEvent::KnowledgeChanged(_) => {
                self.search_invalidate(SearchCategory::Knowledge);
                self.search_invalidate(SearchCategory::KnowledgeHistory);
            }
            UiEvent::Agents(list) => {
                // Reseeding keeps what events taught about rows still there.
                let mut old = std::mem::take(&mut self.agents);
                self.agents = list
                    .into_iter()
                    // 125 §7.5: the hidden advisor child is never a board row.
                    .filter(|info| info.kind != SessionKind::Advisor)
                    .map(|info| {
                        let row = match old.remove(&info.id) {
                            Some(mut row) => {
                                row.info = info;
                                row
                            }
                            None => AgentRow::new(info),
                        };
                        (row.info.id, row)
                    })
                    .collect();
                // The owned tree's roster: navigating within it is not a leave (51 §4.3).
                if let Some(p) = &mut self.private
                    && self.session.is_some_and(|s| p.member(s))
                {
                    p.members.extend(self.agents.keys().copied());
                }
                self.approvals
                    .retain(|a| Some(a.session) == self.session || self.agents.contains_key(&a.session));
                if !std::mem::take(&mut self.agents_picker_pending) {
                    return;
                }
                // The picker shows errors and cancels; the board lets go of them.
                for row in self.agents.values_mut() {
                    row.fresh = false;
                }
                if self.agents.is_empty() {
                    self.entries.push(Entry::Info("no subagents".into()));
                } else {
                    self.menu = Some(Menu::Agents {
                        items: sort_agents(self.agents.values().cloned().collect()),
                        cursor: 0,
                    });
                }
            }
            UiEvent::History(id, loaded) => {
                if self.session == Some(id) {
                    self.load(loaded);
                }
            }
            UiEvent::Refreshed(id, loaded) => {
                // An inspected page is not the live transcript: no reload stitches over it.
                if self.session == Some(id) && self.inspect.is_none() {
                    self.refresh(loaded);
                }
            }
            UiEvent::ContextItems {
                session,
                category,
                snapshot,
            } => self.open_shake(session, category, snapshot),
            UiEvent::Activity(list) => {
                self.menu = Some(Menu::Broadcast(Box::new(Broadcast::pick(list))));
            }
            UiEvent::Broadcasted(r) => self.broadcasted(r),
            // A failed read leaves the plan on screen, labelled stale (47 §5.4).
            UiEvent::Todos(id, snapshot) => {
                if self.session == Some(id) {
                    self.loaded_todos(snapshot);
                }
            }
            // The transcript reload that follows pushes the notice (`Refreshed`).
            UiEvent::ContextRemoved(id, r) => {
                if self.session == Some(id) {
                    self.notice = Some(removal_line(&r));
                }
            }
            UiEvent::Forked(id, loaded) => {
                self.reset();
                self.session = Some(id);
                self.notice = Some("forked — /rename to tell them apart".into());
                self.load(loaded);
            }
            UiEvent::SessionCreated(id) => {
                self.awaiting_session = false;
                // The draft's picks rode on the creation (148 R1); they are this session's now.
                self.draft = None;
                if self.session.is_none() {
                    self.session = Some(id);
                    // The global default was shown; now this session's own view.
                    self.refresh_communication = true;
                }
                if let Some((s, message)) = self.send_error.take()
                    && s == id
                {
                    self.entries.push(Entry::Error(message));
                }
            }
            UiEvent::Models(items) => {
                let target = std::mem::take(&mut self.pick_target);
                if items.is_empty() {
                    self.entries
                        .push(Entry::Error("no models: no provider is configured".into()));
                } else {
                    // One provider only: skip the providers level (124 §3.5).
                    let provider = match providers(&items).as_slice() {
                        [(p, _)] => Some(p.clone()),
                        _ => None,
                    };
                    self.menu = Some(Menu::Models {
                        items,
                        filter: String::new(),
                        cursor: 0,
                        provider,
                        target,
                    });
                    self.models_home();
                }
            }
            UiEvent::PathComplete(generation, reply) => self.path.reply(generation, reply),
            // Chained picker: a model with effort levels asks for one right away (Esc skips).
            UiEvent::ModelSet(info) => {
                self.model_id = info.id;
                if !info.efforts.is_empty() {
                    self.open_efforts(info.efforts, PickTarget::Session);
                }
            }
            UiEvent::FavoriteModels(models) => self.set_favorites(models),
            UiEvent::EffortSet(effort) => {
                self.awaiting_effort = false;
                self.entries.push(Entry::Info(match &effort {
                    Some(e) => format!("effort: {e}"),
                    None => "effort cleared".to_owned(),
                }));
                self.effort = effort;
            }
            // The reply of `/advisor …` or a load's refresh (125 §7.1): installed
            // silently; `status` prints its lines; a pick chains into the effort picker.
            UiEvent::AdvisorStatus(status) => {
                if status.session != self.session.unwrap_or(status.session) {
                    return;
                }
                let why = self.advisor_reply.take();
                self.set_advisor_status(status);
                match why {
                    Some(AdvisorReply::Status) => {
                        for line in advisor_status_lines(&self.advisor) {
                            self.entries.push(Entry::Info(line));
                        }
                    }
                    Some(AdvisorReply::Pick) => {
                        let levels = self
                            .advisor
                            .model
                            .as_deref()
                            .zip(self.catalog.as_ref())
                            .and_then(|(id, c)| c.closest(id))
                            .map(|m| m.efforts())
                            .unwrap_or_default();
                        if !levels.is_empty() {
                            self.open_efforts(levels, PickTarget::Advisor);
                        }
                    }
                    None => {}
                }
            }
            UiEvent::HarnessOptions(options) => {
                for line in harness_options_lines(&options) {
                    self.entries.push(Entry::Info(line));
                }
            }
            UiEvent::HarnessOptionSet(key, value, r) => {
                self.entries
                    .push(Entry::Info(harness_option_set_line(&key, &value, &r)));
            }
            // The cursor is clamped where it is used (`mcp_key`, renderer).
            UiEvent::Mcp(list) => match &mut self.menu {
                Some(Menu::Mcp { servers, .. }) => *servers = list,
                _ if self.awaiting_mcp => {
                    self.awaiting_mcp = false;
                    self.menu = Some(Menu::Mcp {
                        servers: list,
                        cursor: 0,
                        expanded: BTreeSet::new(),
                        pending: None,
                    });
                }
                _ => {}
            },
            UiEvent::Acp(list) => match &mut self.menu {
                Some(Menu::Acp { entries, .. }) => *entries = list,
                _ if self.awaiting_acp => {
                    self.awaiting_acp = false;
                    self.menu = Some(Menu::Acp {
                        entries: list,
                        registry: Vec::new(),
                        cursor: 0,
                    });
                }
                _ => {}
            },
            UiEvent::AcpRegistry(list) => {
                if let Some(Menu::Acp { registry, .. }) = &mut self.menu {
                    *registry = list;
                }
            }
            UiEvent::AcpAdded(e) => self.entries.push(Entry::Info(format!(
                "acp: added `{}` ({} {})",
                e.name,
                e.command,
                e.args.join(" ")
            ))),
            UiEvent::Approvals(s) => {
                self.approvals_global = s.global;
                self.entries.push(Entry::Info(format!(
                    "approvals: {} (this session), {} (default)",
                    on_off(s.session && !self.approvals_off),
                    on_off(s.global)
                )));
            }
            UiEvent::Communication(s) => {
                // A reply for the session shown when it was asked, not now.
                if s.session != self.session {
                    return;
                }
                let print = std::mem::take(&mut self.awaiting_communication);
                if self.set_communication(s) && print {
                    let lines = communication_lines(self.communication.as_ref().expect("just set"));
                    self.entries.extend(lines.into_iter().map(Entry::Info));
                }
            }
            UiEvent::Peers(session, page) => {
                if let Some(Menu::Peers {
                    session: s, peers, ..
                }) = &mut self.menu
                    && *s == session
                {
                    *peers = Some(page);
                }
            }
            UiEvent::PeerRequests(session, page) => {
                if let Some(Menu::Peers {
                    session: s,
                    requests,
                    ..
                }) = &mut self.menu
                    && *s == session
                {
                    *requests = Some(page);
                }
            }
            UiEvent::PeerAnswered(r) => self.entries.push(Entry::Info(format!(
                "peers: {} answered → {}{}{}",
                short_uuid(r.reply_to.unwrap_or(r.id)),
                r.state.as_str(),
                r.answered_by
                    .as_deref()
                    .map_or_else(String::new, |by| format!(" · by {by}")),
                r.error
                    .as_deref()
                    .map_or_else(String::new, |e| format!(" · {e}")),
            ))),
            UiEvent::A2a(status) => self
                .entries
                .extend(a2a_lines(&status).into_iter().map(Entry::Info)),
            // Any subtree may be an ancestor of ours: the state is one cheap
            // call, so every change reloads it (50 §10). The open picker's
            // directory follows the switches too.
            UiEvent::CommunicationChanged(_) | UiEvent::A2aChanged(_) => {
                self.refresh_communication = true;
                self.refresh_peers |= matches!(self.menu, Some(Menu::Peers { .. }));
            }
            UiEvent::PeerMessageChanged(ev) => {
                self.refresh_peers |=
                    matches!(&self.menu, Some(Menu::Peers { session, .. }) if *session == ev.session);
            }
            UiEvent::PeerDelivered(ev) => {
                if self.session == Some(ev.session) {
                    self.remember_peer(&ev.text);
                    self.interject(peer_card(&ev.origin, ev.text, ""));
                }
            }
            UiEvent::McpProgress(p) => {
                // Any change while the menu is open (a login settled, a server
                // (dis)connected) re-lists; waiting for `connecting` to drain
                // left a finished login unshown while a slow server reconnected.
                if self.mcp.as_ref() != Some(&p) && matches!(self.menu, Some(Menu::Mcp { .. })) {
                    self.refresh_mcp = true;
                }
                self.mcp = Some(p);
            }
            // Verbatim: the point is to see exactly what the model sees.
            UiEvent::Prompt(text) => self.entries.push(card(
                CardKind::Prompt,
                "prompt".into(),
                &format!("```\n{text}\n```"),
            )),
            UiEvent::Skills(items) => {
                self.skills = items.clone();
                if !std::mem::take(&mut self.skills_picker_pending) {
                    return;
                }
                if items.is_empty() {
                    self.entries.push(Entry::Info("no skills".into()));
                } else {
                    self.menu = Some(Menu::Skills { items, cursor: 0 });
                }
            }
            UiEvent::Logins(items) => {
                if items.is_empty() {
                    self.entries
                        .push(Entry::Info("nothing to log in to".into()));
                } else {
                    self.menu = Some(Menu::Logins { items, cursor: 0 });
                }
            }
            UiEvent::LoginFinished(f) => self.entries.push(match f.error {
                None => Entry::Info(format!("login: {} ok", f.provider)),
                Some(e) => Entry::Error(format!("login: {} failed: {e}", f.provider)),
            }),
            UiEvent::Skill(s) => {
                let text = if s.files.is_empty() {
                    s.body
                } else {
                    format!("{}\n\n---\nfiles: {}", s.body, s.files.join(", "))
                };
                self.entries.push(card(CardKind::Skill, s.name, &text));
            }
            UiEvent::SkillsAdded(items) => self.entries.push(Entry::Info(if items.is_empty() {
                "cloned; no skills found".into()
            } else {
                format!(
                    "added {} skills: {}",
                    items.len(),
                    items
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })),
            UiEvent::Synced(results) => {
                if results.is_empty() {
                    self.entries.push(Entry::Info("no repos".into()));
                }
                for r in results {
                    self.entries
                        .push(Entry::Info(format!("{}  {}", r.name, r.status)));
                }
            }
            UiEvent::McpAdded(s) => self.entries.push(Entry::Info(format!(
                "{}  {} tools  {}",
                s.name,
                s.tools.len(),
                if s.connected {
                    "connected"
                } else {
                    "not connected"
                }
            ))),
            UiEvent::RepoAdded(r) => self.entries.push(Entry::Info(format!(
                "cloned {} → {}  (build it, then `/mcp add {{...\"cwd\": ...}}`)",
                r.name, r.path
            ))),
            // Always the picker: `H` reaches the bundle's history (and so
            // deleted concepts) even when nothing is left to list.
            UiEvent::Knowledge(items) => self.menu = Some(Menu::Knowledge { items, cursor: 0 }),
            // Fills the menu opened by the request it answers; anything else
            // (closed with esc, another scope or page since) is dropped.
            UiEvent::KnowledgeHistory {
                id,
                limit,
                history,
            } => {
                if let Some(Menu::KnowledgeHistory {
                    id: open,
                    changes: shown,
                    limit: page,
                    more,
                    loading,
                    error,
                    ..
                }) = &mut self.menu
                    && *open == id
                    && *page == limit
                {
                    let mut changes = history.changes;
                    *more = changes.len() > limit;
                    changes.truncate(limit);
                    *shown = changes;
                    *loading = false;
                    *error = None;
                }
            }
            UiEvent::KnowledgeRevision {
                id,
                revision,
                card: true,
            } => {
                let title = format!(
                    "knowledge {}  {}{}",
                    revision.change.commit,
                    revision.change.message,
                    id.map_or(String::new(), |i| format!("  ({i})"))
                );
                let text = revision_rows(&revision).join("\n");
                self.entries.push(card(
                    CardKind::Skill,
                    title,
                    &format!("```diff\n{text}\n```"),
                ));
            }
            // Only the commit still awaited in the open scope.
            UiEvent::KnowledgeRevision {
                id,
                revision,
                card: false,
            } => {
                if let Some(Menu::KnowledgeHistory {
                    id: open,
                    pending,
                    revision: shown,
                    ..
                }) = &mut self.menu
                    && *open == id
                    && pending.as_deref() == Some(revision.change.commit.as_str())
                {
                    *pending = None;
                    *shown = Some((revision, 0, 0));
                }
            }
            UiEvent::KnowledgeRestored(c) => self
                .entries
                .push(Entry::Info(format!("{}  {}", c.commit, c.message))),
            UiEvent::Harvested(r) => self.entries.push(Entry::Info(format!(
                "knowledge +{} ~{}",
                r.added, r.updated
            ))),
            UiEvent::Info(t) => self.entries.push(Entry::Info(t)),
            // The note rides the reload (like undo's), so it lands after the
            // transcript the new engine serves — a pushed entry would go with
            // the old one.
            UiEvent::Engine(attached) => {
                let note = match &attached {
                    Some(label) => label.clone(),
                    None => format!("node process started here (pid {})", std::process::id()),
                };
                self.attached = attached;
                match self.session {
                    Some(_) => {
                        self.notice = Some(note);
                        self.reselect = true;
                    }
                    None => self.entries.push(Entry::Info(note)),
                }
            }
            UiEvent::Jobs(jobs) => self.entries.push(Entry::Info(tools::jobs::table(&jobs))),
            UiEvent::ArtifactShown(ev) => {
                self.search_invalidate(SearchCategory::Artifacts);
                if self.session == Some(ev.session) {
                    self.attach_artifact(ev.artifact);
                }
            }
            UiEvent::Artifacts {
                items,
                pick,
                apps,
                sessions,
            } => {
                if !pick {
                    for a in items {
                        self.attach_artifact(a);
                    }
                } else if items.is_empty() {
                    self.entries.push(Entry::Info(
                        if apps { "no apps" } else { "no artifacts" }.into(),
                    ));
                } else {
                    let items = artifact_rows(items, |id| self.owner(&sessions, id));
                    let cursor = match &self.menu {
                        Some(Menu::Artifacts { cursor, .. }) => *cursor,
                        _ => grouped_first(&items),
                    };
                    self.menu = Some(Menu::Artifacts {
                        items,
                        cursor,
                        apps,
                    });
                }
            }
            UiEvent::Job(ev) => match ev {
                JobEvent::Started {
                    session,
                    id,
                    call_id,
                    ..
                } if self.session == Some(session) => {
                    if let Some(Entry::Tool { job, .. }) = self.entries.iter_mut().rev().find(
                        |e| matches!(e, Entry::Tool { call_id: c, .. } if *c == call_id),
                    ) {
                        *job = Some(JobTail::new(id.0.clone()));
                    }
                    self.jobs.insert(id.0);
                }
                JobEvent::Output { session, id, chunk } if self.session == Some(session) => {
                    if let Some(j) = self.job_mut(&id.0) {
                        j.push(&chunk);
                    }
                }
                JobEvent::Screen {
                    session, id, rows, ..
                } if self.session == Some(session) => {
                    if let Some(j) = self.job_mut(&id.0) {
                        j.set_screen(&rows);
                    }
                }
                JobEvent::Finished {
                    session,
                    id,
                    exit,
                    secs,
                    inline,
                    ..
                } if self.session == Some(session) => {
                    match self.entries.iter_mut().rev().find(
                        |e| matches!(e, Entry::Tool { job: Some(j), .. } if j.id == id.0),
                    ) {
                        // Within grace: the result carries the output.
                        Some(Entry::Tool { job, .. }) if inline => *job = None,
                        Some(Entry::Tool { job: Some(j), .. }) => {
                            j.exit = Some(format!("{exit} · {secs}s"));
                        }
                        _ => {}
                    }
                    self.jobs.remove(&id.0);
                }
                _ => {}
            },
            UiEvent::CompactStyle(s) => self.entries.push(Entry::Info(format!(
                "compact style: {s} (default)\n  summary  cache-hot LLM fold (07 §8.3)\n  cut      drop the folded span, keep the tail, no model call\n  snap     rasterize the folded span to images (vision model)"
            ))),
            UiEvent::TitleModel(m) => self.entries.push(Entry::Info(title_model_line(m.as_ref()))),
            // The startup fetch only primes the attach notice; `/vision` prints.
            UiEvent::VisionModel(m) => {
                if std::mem::take(&mut self.awaiting_vision) {
                    self.entries
                        .push(Entry::Info(vision_model_line(m.as_ref())));
                }
                self.vision_model = m;
            }
            // Status always; the picker when there is a choice to make or a
            // stale selection to clear (row 0 restores the default chain).
            UiEvent::AwsProfiles { info, pick } => {
                self.entries.push(Entry::Info(aws_profile_line(&info)));
                if pick && (info.profiles.len() > 1 || info.selected.is_some()) {
                    self.menu = Some(Menu::AwsProfiles { info, cursor: 0 });
                }
            }
            UiEvent::Providers(list) => {
                // 85 §0.9: the built-ins come first; there may be no custom provider.
                for p in &list {
                    self.entries.push(Entry::Info(if p.builtin {
                        format!(
                            "{}  built-in  {} models  {}",
                            p.name,
                            p.models,
                            if p.configured {
                                "key set".to_owned()
                            } else {
                                format!("no key — /secrets add {}", p.name)
                            }
                        )
                    } else {
                        format!("{}  {}  {} models", p.name, p.base_url, p.models)
                    }));
                }
                if list.iter().all(|p| p.builtin) {
                    self.entries.push(Entry::Info(
                        "no custom providers; /provider add to create one".into(),
                    ));
                }
            }
            UiEvent::ProviderAdded(p) => self.entries.push(Entry::Info(format!(
                "provider {}  {}  {} models — /model {}/<id>",
                p.name, p.base_url, p.models, p.name
            ))),
            UiEvent::Withdrawn { id, text } => {
                self.queued.retain(|q| q.id != id);
                self.recall = Some(text);
            }
            UiEvent::Unsent { id, text } => {
                match id {
                    Some(id) => self.queued.retain(|q| q.id != id),
                    // The optimistic row from `submit_with`.
                    None => {
                        if let Some(i) = self
                            .entries
                            .iter()
                            .rposition(|e| matches!(e, Entry::User { turn: None, .. }))
                        {
                            self.entries.remove(i);
                        }
                        self.sent = None;
                        self.awaiting_session = false;
                    }
                }
                self.recall = Some(text);
            }
            // The transcript reload that follows pushes the notice (`History`).
            UiEvent::Undone(t) => self.notice = Some(format!("undone: {t} — /redo restores")),
            UiEvent::Redone(t) => self.notice = Some(format!("restored: {t}")),
            UiEvent::MeshDevices(list) => {
                if list.is_empty() {
                    self.entries.push(Entry::Info("mesh devices: none paired".into()));
                }
                let now = now_ms();
                for d in list {
                    self.entries.push(Entry::Info(mesh_device_line(&d, now)));
                }
            }
            UiEvent::MeshDevicePairing(p) => {
                // The QR any engine may show; the hub accepts (69 decision 4). Shares the
                // node-invite countdown slot: one ticking invite card at a time.
                let card = PairingCard::new(link::Pairing {
                    key: p.string,
                    expires_at_ms: p.expires_ms,
                });
                self.entries.push(Entry::Card {
                    kind: CardKind::Component { group: "node-invite", key: card.id, glyph: "⌁", color: crate::ui::FAIENCE },
                    title: format!("mesh device pairing · pending {}", p.pending_id),
                    text: Box::new(markdown::Text::complete(&card.body())),
                    expanded: true,
                });
                self.invite = Some(card);
            }
            UiEvent::Secrets(list) => {
                if self.awaiting_secrets && !matches!(self.menu, Some(Menu::Secrets { .. })) {
                    self.menu = Some(Menu::Secrets {
                        cursor: 0,
                        history: None,
                    });
                }
                self.awaiting_secrets = false;
                self.secrets = Some(list);
            }
            UiEvent::SecretSet(info) => self
                .entries
                .push(Entry::Info(format!("{}: v{} set", info.name, info.current))),
            UiEvent::SecretsEvent(ev) => self.apply_secrets(ev),
            UiEvent::Error(e) => {
                // A failed `/compact` answers here, not with an agent event.
                self.compacting = false;
                self.awaiting_effort = false;
                self.awaiting_vision = false;
                self.awaiting_secrets = false;

                self.notice = None;
                // A knowledge request that was in flight failed: the menu
                // shows it where the rows would be (`r` retries).
                if let Some(Menu::KnowledgeHistory {
                    loading,
                    pending,
                    error,
                    ..
                }) = &mut self.menu
                    && (*loading || pending.is_some())
                {
                    *loading = false;
                    *pending = None;
                    *error = Some(e);
                    return;
                }
                // 139 §9.3: the owner-offline line says when `/take` also resumes a turn.
                let e = match &self.owner_turn {
                    Some(owner) if e.ends_with("— /take") => format!("{e}{}", owner_turn_suffix(owner)),
                    _ => e,
                };
                self.entries.push(Entry::Error(e));
            }
            UiEvent::Terminals(items) => {
                if items.is_empty() {
                    self.entries
                        .push(Entry::Info("no terminals in this session's tree".into()));
                } else {
                    self.menu = Some(Menu::Terminals { items, cursor: 0 });
                }
            }
            UiEvent::PtyScreen {
                session,
                job,
                viewport,
                epoch,
                reply,
            } => {
                if let Some(Menu::Terminal(v)) = &mut self.menu {
                    v.reply(session, &job, viewport, epoch, reply);
                }
            }
            UiEvent::SessionOpened(open) => {
                self.reset();
                self.session = Some(open.session);
                self.refresh_communication = true;
                if open.retention == Retention::Private
                    && let Some(secret) = open.lease_secret
                {
                    self.private = Some(Private {
                        root: open.root,
                        secret,
                        members: std::collections::HashSet::new(),
                    });
                    if !std::mem::replace(&mut self.private_warned, true) {
                        self.entries.push(Entry::Info(PRIVATE_WARNING.into()));
                    }
                    self.entries.push(Entry::Info(format!(
                        "private session {} — leaving it (/new, /sessions, /resume, /quit) closes the tree",
                        short(open.root)
                    )));
                } else {
                    self.entries.push(Entry::Info("new session".into()));
                }
            }
            UiEvent::SessionLeft(_, result) => {
                self.entries.push(Entry::Info(
                    match result.cleanup {
                        CleanupStatus::Complete => "private session closed",
                        CleanupStatus::Pending => "private session closed (cleanup pending)",
                        CleanupStatus::Incomplete => "private session closed (cleanup incomplete)",
                    }
                    .into(),
                ));
            }
            UiEvent::Voice(ev) => self.apply_voice(ev),
            UiEvent::Talk(ev) => self.apply_talk(ev),
            UiEvent::DictationStarting(id) => {
                // `/new` or a menu may have run between the key and the reply; an
                // empty anchor still tracks the dictation the daemon opened for us.
                let (before, after) = self.dictation_pending.take().unwrap_or_default();
                self.dictation = Some(Dictating::new(id, before, after));
            }
            UiEvent::DictationFailed(m) => {
                self.dictation_pending = None;
                self.dictation = None;
                self.entries.push(Entry::Error(format!("dictation: {m}")));
            }
            UiEvent::TalkStarting(id) => {
                self.talk_pending = false;
                // `SessionOpened` came first when `/talk` had to create the session.
                let Some(session) = self.session else { return };
                self.talk = Some(Talking::new(id, session));
                self.talk_view = true;
            }
            UiEvent::TalkFailed(m) => {
                self.talk_pending = false;
                self.entries.push(Entry::Error(format!("talk: {m}")));
            }
        }
    }
}
