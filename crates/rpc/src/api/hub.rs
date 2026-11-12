use super::*;
use sessionsync::rpc_support::RpcSessionMoveExt;
use super::sessions::SessionStateExt;

#[rpc(server, client)]
pub trait CoreApi {
    /// Version, data dir, default model, approvals and compaction style in one
    /// call: the client's startup snapshot.
    #[method(name = "info")]
    async fn info(&self) -> RpcResult<Info>;
    /// PATH-AC: candidates for the path token under `cursor` in `input`,
    /// listed by the engine running `session` (`None`: this engine, its
    /// cwd). Relative paths resolve against the session's cwd; `~`/`$VAR`
    /// expand for the lookup only. Empty when nothing triggers, the
    /// directory is missing or the owner is unreachable: never an error.
    /// 183 W-12: `engine` (mesh id) lists on that engine when `session` is
    /// `None` (New chat on a machine); a missing trailing param is `None`.
    #[method(name = "path_complete", with_extensions)]
    async fn path_complete(
        &self,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
        engine: Option<String>,
    ) -> RpcResult<PathCompletion>;
    /// 191 (D3): a contact's unary call (`POST path` with the JSON-RPC `body`) that another
    /// machine of the mesh received for a session owned here: this machine checks `caller`
    /// itself and answers it in its own a2a router. Mesh-internal like `peer_deliver`: a call
    /// without the `Forwarded` marker is `-32002`. `NOT_OWNER` (-32013) when the session is
    /// not owned here either, `OWNER_UNREACHABLE` (-32010) before `contacts_ready`; every
    /// outcome of the router is `{status, body}`.
    #[method(name = "a2a_forward", with_extensions)]
    async fn a2a_forward(
        &self,
        caller: String,
        path: String,
        body: String,
    ) -> RpcResult<agent::communication::mesh::A2aAnswer>;
    /// PTY jobs of `session` (and, with `descendants: true`, its subtree)
    /// whose terminal is still retained (51 §7.3).
    #[method(name = "list_terminals", with_extensions)]
    async fn list_terminals(
        &self,
        session: SessionId,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<TerminalSummary>>;
    /// Archive or unarchive a root; archived trees leave the sidebar but stay
    /// in `list_sessions` (flagged) and search (51 §3).
    #[method(name = "archive_session", with_extensions)]
    async fn archive_session(&self, session: SessionId, archived: bool) -> RpcResult<()>;
    /// 181: send `session`, owned here, to the paired engine `to` (its mesh
    /// id): `to` takes it (`session_take(moved)` over the mesh), this engine
    /// releases it, a running turn resumes there. Errors in plain words
    /// (`xeus is offline`, `this session is not owned here`, `xeus refused: …`).
    #[method(name = "session_move", with_extensions)]
    async fn session_move(&self, session: SessionId, to: String) -> RpcResult<SessionOwner>;
    /// Who owns `session` and whether that is this engine (68 §5).
    #[method(name = "session_owner", with_extensions)]
    async fn session_owner(&self, session: SessionId) -> RpcResult<SessionOwner>;
    /// Turns this engine wrote as a superseded owner and never delivered
    /// (68 §4.3), one entry per epoch; empty when none.
    #[method(name = "session_orphans", with_extensions)]
    async fn session_orphans(&self, session: SessionId) -> RpcResult<Vec<SessionOrphan>>;
    /// Open a root with explicit storage (51 §4.1): `retention: durable`
    /// (default) is `new_session` plus `project`; `private` needs `view` and
    /// binds the tree to the calling connection, returning the lease secret
    /// it presents on `resume_private` / `leave_session`. Always returns
    /// `SessionOpen`.
    #[method(name = "create_session", with_extensions)]
    async fn create_session(&self, options: CreateSessionOptions) -> RpcResult<SessionOpen>;
    /// Unified human search (design doc 49 §4.2) over sessions, artifacts,
    /// current knowledge and knowledge history: one group per selected category
    /// in fixed order, each `ready`/`indexing`/`unavailable`/`error` on its own.
    /// `categories` omitted → all four, `[]` → none; `limit` per category,
    /// default 10, max 50. Blank query → no groups, no cache work.
    /// Clients re-ask only `indexing` groups (`categories: [that]`) every 500 ms.
    #[method(name = "search")]
    async fn search(
        &self,
        query: String,
        categories: Option<Vec<SearchCategory>>,
        limit: Option<u32>,
    ) -> RpcResult<SearchResponse>;
    /// Retry `search`'s failed owners without matching: the selected owners
    /// (omitted → all four) drop failed/stale search build state so the next
    /// `search` rebuilds; knowledge also re-syncs current concepts with disk.
    /// Owners not running are skipped; any failing reset is the error.
    #[method(name = "search_refresh")]
    async fn search_refresh(&self, categories: Option<Vec<SearchCategory>>) -> RpcResult<()>;
    /// Answer a pending approval; `true` if it was still pending.
    #[method(name = "resolve_approval", with_extensions)]
    async fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> RpcResult<bool>;
    /// Answer a pending `ask_user`: one answer per question, or `None` to
    /// dismiss the set, which also cancels the turn. `true` if it was still pending.
    #[method(name = "answer_ask", with_extensions)]
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> RpcResult<bool>;
    /// 183 §4.5 (W-5): the approvals switches as they stand, for `session`
    /// (answered by its owner) or the global default with `None`.
    #[method(name = "get_approvals", with_extensions)]
    async fn get_approvals(&self, session: Option<SessionId>) -> RpcResult<ApprovalsState>;
    /// The document bound to `binding`. A knowledge binding whose concept
    /// exists but has no document yet gets one created from its file (59
    /// §7.1, on demand), an agent-charter binding from the roster file's body;
    /// `None` only when there is nothing to bind to.
    #[method(name = "doc_resolve", with_extensions)]
    async fn doc_resolve(&self, binding: document::Binding) -> RpcResult<Option<document::DocId>>;
    /// Export `doc` (72): `markdown` = `doc_markdown`'s text, `pdf` = the print lowering through Typst
    /// with `assets` (client-rendered diagram SVGs by block id) as figures. `dest` (owner only, under
    /// the home directory) is written owner-only and atomically; `None` stores an artifact instead.
    #[method(name = "doc_export", with_extensions)]
    async fn doc_export(
        &self,
        doc: document::DocId,
        format: artifact::ExportFormat,
        dest: Option<PathBuf>,
        assets: Vec<artifact::ExportAsset>,
    ) -> RpcResult<artifact::ExportResult>;
    /// 184 §4.8: "Move to ▸ machine" (owner only): the note's server writes `home = to` and hands the
    /// live copy to `to`. Notes only; boards, meeting notes, task threads and unhomed kinds are refused.
    #[method(name = "doc_move_home", with_extensions)]
    async fn doc_move_home(
        &self,
        doc: document::DocId,
        to: String,
    ) -> RpcResult<document::DocSummary>;
    /// Remove a member (59 §8.4; owner only): `Closed{revoked}`, their connections dropped, the next
    /// handshake from that static finds no membership.
    #[method(name = "doc_member_revoke", with_extensions)]
    async fn doc_member_revoke(&self, doc: document::DocId, member: String) -> RpcResult<()>;
    /// A new share link (59 §8.2; owner only): `https://<relay>/d#1.<mesh device id | engine id>.<doc>.<secret>` (69),
    /// returned once — the harness keeps only `sha256(secret)`. Needs Link enabled (the relay host).
    #[method(name = "doc_link_create", with_extensions)]
    async fn doc_link_create(
        &self,
        doc: document::DocId,
        role: document::Role,
        name: Option<String>,
        expires_ms: Option<i64>,
    ) -> RpcResult<document::LinkCreated>;
    /// Add a source to a notebook (144 §4): pre-flight, the `new` row and card at once, then the ingest job
    /// (`SourceEvent` on `subscribe_events`). Exactly one origin field. Hub only.
    #[method(name = "source_add", with_extensions)]
    async fn source_add(
        &self,
        args: document::ingest::SourceAddArgs,
    ) -> RpcResult<document::Source>;
    /// Re-run the ingest from the origin or the blob (144 §4.2); refused while queued or running. Hub only.
    #[method(name = "source_refresh", with_extensions)]
    async fn source_refresh(&self, id: document::DocId) -> RpcResult<document::Source>;
    /// The three builtins, then the skills whose front matter has `notebook_insight: true`.
    #[method(name = "insight_skills")]
    async fn insight_skills(&self) -> RpcResult<Vec<String>>;
    /// Stop the whole process. Logged with the calling connection (79 smoke bug 7).
    #[method(name = "shutdown", with_extensions)]
    async fn shutdown(&self) -> RpcResult<()>;
    /// 153 F1: the live user-facing `voice.*` settings — what `set_shape`, `set_end_phrases`,
    /// `set_input_devices` and `set_polish_model` left behind, plus the config-only keys.
    #[method(name = "voice_settings")]
    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings>;
    /// Settings › Search (16 §8): the four external providers (enabled, base
    /// URL, secret name and whether it is set - never the value, timeout,
    /// priority, registered) plus the `web` read/budget defaults.
    #[method(name = "search_settings_get")]
    async fn search_settings_get(&self) -> RpcResult<search_providers::SearchSettings>;
    /// Persist `search_providers.*` and the `web` defaults to config.yaml and
    /// apply them live: providers are re-registered with the `web` actor, the
    /// next `search_web` uses the new limits. A failed write changes nothing.
    #[method(name = "search_settings_set")]
    async fn search_settings_set(
        &self,
        settings: search_providers::SearchSettings,
    ) -> RpcResult<search_providers::SearchSettings>;
    /// Leave an engine out of the mesh (67 §4.1); 183 D7: the remaining machines then rotate
    /// the vault key. `reset_devices` (D11, default false): also rotate `mesh/device_key`
    /// after the vault key, so every phone and iPad re-pairs.
    #[method(name = "mesh_remove")]
    async fn mesh_remove(&self, id: String, reset_devices: Option<bool>) -> RpcResult<()>;
    /// 183 §12.9: what unpairing `id` means, for the Unpair dialog.
    #[method(name = "mesh_unpair_preview")]
    async fn mesh_unpair_preview(&self, id: String) -> RpcResult<mesh::UnpairPreview>;
    /// 182 D10: the task lives on engine `to` (a mesh id) from now on — only its desk works it there.
    /// `to` must be online and have the task's agent (theuth is everywhere); a done or cancelled
    /// task does not move. A live work session of its checkout owned here moves first
    /// (`session_move`); that move's refusal refuses this one. Off an offline home the move
    /// lands here at once and replicates when that engine returns.
    #[method(name = "task_move_home", with_extensions)]
    async fn task_move_home(&self, id: String, to: String) -> RpcResult<tasks::Task>;
    /// 182 D9 "Move them to <this engine>": every task waiting on the offline engine `from`
    /// moves to this engine; the ones whose agent is not here are named in `refused`.
    #[method(name = "task_move_home_waiting", with_extensions)]
    async fn task_move_home_waiting(&self, from: String) -> RpcResult<tasks::MovedHome>;
    /// A committed app record: its artifact, source, compatibility hash,
    /// pinned tool bindings and diagnostics (46 §6.3, §8.3). Rejects with
    /// `-32001` when the app plugin is disabled: the client falls back to
    /// the artifact's readable source. A private record needs its owning
    /// `session` (authorized, 51 §4.2).
    #[method(name = "get_app", with_extensions)]
    async fn get_app(
        &self,
        record_call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<AppDoc>;
    /// 129 §3.2: per-connection focus. Delta-class events (`TextDelta`, `ReasoningDelta`,
    /// `ToolCallProgress`, `VisionProgress`) reach this connection's `subscribe_events` streams
    /// only for `sessions`; every other event is unaffected. A connection that never calls this
    /// receives everything. Replaces the previous set; `[]` means no deltas at all. The set dies
    /// with the connection.
    #[method(name = "set_event_focus", with_extensions)]
    async fn set_event_focus(&self, sessions: Vec<SessionId>) -> RpcResult<()>;
    /// Stream every wire-visible event as `{"type": ..., ...}`, optionally
    /// only those carrying `session`. `descendants: true` (default `false`)
    /// also delivers events of every session in `session`'s subtree, plus
    /// `SubagentSpawned` whose `parent` is in it (19 §2.2). The first item is
    /// always a [`PendingPrompts`] snapshot; a client replaces its pending
    /// prompt set with it. A slow client sees `{"type":"Lagged","count":n}`
    /// where `n` non-delta events were dropped for it (129 §3.3), followed by
    /// a fresh snapshot; delta-class events are merged while they wait and
    /// are never lost that way. Deltas of sessions outside `set_event_focus`
    /// are not delivered.
    #[subscription(name = "subscribe_events", unsubscribe = "unsubscribe_events", item = serde_json::Value, with_extensions)]
    async fn subscribe_events(
        &self,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult;
    /// 176 C1: liveness only — answers `null` at once. Asks no actor and awaits nothing, so a
    /// busy engine still answers; probe with this, not `info`, to tell a dead socket from a
    /// slow handler.
    #[method(name = "ping")]
    fn ping(&self) -> RpcResult<()>;
    /// 176 C2: one client diagnostic line into the engine log, target `client`, fields
    /// `source` and `conn` (the `rpc: ws open conn=<n>` id; 0 off the local WebSocket).
    /// `level` `"warn"` logs at WARN, anything else at INFO; `source` is ≤32 chars of
    /// `[a-z0-9-]` (else logged as `-`); `message` is cut to 2,000 chars. 20 per minute per
    /// connection; the rest of that minute is dropped with one WARN. Asks no actor.
    #[method(name = "client_log", with_extensions)]
    fn client_log(&self, level: String, source: String, message: String) -> RpcResult<()>;
}

#[async_trait]
impl CoreApiServer for Server {
    async fn info(&self) -> RpcResult<Info> {
        // 82 §3 / 83 §3: the supervisor's live defaults, not the config file read at startup.
        // 176 E3: one ask, so `info` queues behind at most one supervisor message.
        let agent::InfoDefaults {
            default_model,
            default_effort,
            favorite_models,
            compact_style,
            subagent_timeout_secs,
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            communication,
            advisor,
        } = ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetInfoDefaults).await?;
        let approvals =
            ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetApprovals { session: None }).await?;
        let pty_idle_secs = ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetPtyIdle).await?;
        let name = match self.rt.section::<LinkConfig>("link") {
            Ok(c) if !c.name.is_empty() => c.name,
            _ => platform::process::hostname().to_owned(),
        };
        // 88: the schedule actor's live value; the config file when it is not running.
        let schedule_run_timeout_secs = match schedule() {
            Ok(s) => send(&s, schedule::GetRunTimeout).await?,
            Err(_) => self
                .rt
                .section::<schedule::ScheduleConfig>("schedule")
                .map(|c| c.run_timeout_secs)
                .unwrap_or(1800),
        };
        Ok(Info {
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            data_dir: self.rt.data_dir.display().to_string(),
            default_model,
            default_effort,
            favorite_models,
            approvals,
            compact_style,
            subagent_timeout_secs,
            schedule_run_timeout_secs,
            home_offline_ask_secs: self
                .rt
                .section::<tasks::TasksConfig>("tasks")
                .map(|c| c.home_offline_ask_secs)
                .unwrap_or_else(|_| tasks::TasksConfig::default().home_offline_ask_secs),
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            pty_idle_secs,
            name,
            communication,
            app_enabled: apps().is_ok(),
            // 125 §6.5: the supervisor's live advisor defaults (82 §3 rule).
            advisor,
        })
    }

    async fn a2a_forward(
        &self,
        ext: &Extensions,
        caller: String,
        path: String,
        body: String,
    ) -> RpcResult<agent::communication::mesh::A2aAnswer> {
        use agent::communication::mesh::ForwardFailure;
        let Some(forward::Forwarded(peer)) = ext.get::<forward::Forwarded>() else {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "a2a_forward is mesh-internal",
                None::<()>,
            ));
        };
        // 183 §6.2 (W-14), 191: `mesh.messaging_off` refuses the forwarder's carried calls as
        // it refused its deliveries; the contact hears words that name no machine.
        if mesh::messaging_off(&peer.0) {
            tracing::info!(forwarder = %peer.0, "191: a2a_forward refused: messaging with this machine is off");
            return Err(ErrorObjectOwned::owned(
                ASK_FAILED,
                A2A_FORWARD_MESSAGING_OFF,
                None::<()>,
            ));
        }
        // Off the a2a mailbox: the plugin's published context, never an actor ask.
        a2a::contacts::forward::serve(&peer.0, &caller, &path, body)
            .await
            .map_err(|e| match e {
                ForwardFailure::NotOwner => ErrorObjectOwned::owned(
                    forward::NOT_OWNER,
                    agent::communication::network::NOT_OWNER_TEXT,
                    None::<()>,
                ),
                ForwardFailure::Offline(why) => {
                    ErrorObjectOwned::owned(forward::OWNER_UNREACHABLE, why, None::<()>)
                }
                ForwardFailure::Refused(why) => {
                    ErrorObjectOwned::owned(ASK_FAILED, why, None::<()>)
                }
            })
    }

    async fn list_terminals(
        &self,
        ext: &Extensions,
        session: SessionId,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<TerminalSummary>> {
        self.authorize(ext, session).await?;
        let mut sessions = vec![session];
        if descendants == Some(true) {
            let agents = ask::<Agents, _>(agent::AGENT_ACTOR, ListAgents { session }).await?;
            sessions.extend(agents.into_iter().map(|a| a.id));
        }
        ask::<Jobs, _>(JOBS_ACTOR, ListTerminals { sessions }).await
    }

    async fn path_complete(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
        engine: Option<String>,
    ) -> RpcResult<PathCompletion> {
        peer(ext)?;
        self.authorize_opt(ext, session).await?;
        // 183 W-12: "Browse xeus…" with no session yet completes on that engine
        // (nothing when it cannot answer, never this machine's paths).
        if session.is_none() {
            match forward::to_engine::<PathCompletion>(
                ext,
                engine.as_deref(),
                "path_complete",
                (None::<SessionId>, &input, cursor),
            )
            .await
            {
                Ok(None) => {}
                Ok(Some(r)) => return Ok(r),
                Err(_) => return Ok(PathCompletion::default()),
            }
        }
        let mut cwd = None;
        if let Some(session) = session {
            if let Some(owner) = forward::owner_peer(session).await {
                let forwarded: RpcResult<Option<PathCompletion>> = forward::to_peer(
                    ext,
                    &owner,
                    "path_complete",
                    (Some(session), &input, cursor),
                )
                .await;
                return Ok(forwarded.ok().flatten().unwrap_or_default());
            }
            let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
            cwd = store
                .session(session)
                .await
                .ok()
                .flatten()
                .and_then(|s| s.cwd)
                .map(PathBuf::from);
        }
        let cwd = cwd
            .or_else(|| std::env::current_dir().ok())
            .or_else(std::env::home_dir)
            .unwrap_or_default();
        Ok(crate::pathac::complete(&cwd, &input, cursor).await)
    }

    async fn session_move(
        &self,
        ext: &Extensions,
        session: SessionId,
        to: String,
    ) -> RpcResult<SessionOwner> {
        RpcSessionMoveExt::session_move(self, ext, session, to).await
    }

    async fn session_owner(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        sessionsync::owner(&store, session)
            .await
            .map_err(|e| failed(format!("{e:#}")))?
            .ok_or_else(|| failed(format!("{session}: no such session")))
    }

    async fn session_orphans(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<SessionOrphan>> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        sessionsync::orphans(&store, session)
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn archive_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        archived: bool,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        match forward::route(ext, session, "archive_session", (session, archived)).await? {
            Some(r) => r,
            None => {
                ask::<Agents, _>(agent::AGENT_ACTOR, ArchiveSession { session, archived }).await?
            }
        }
        // 183 W-41, §12.10: an exported chat's export row reads Paused while it is archived;
        // this engine holds the export (contacts are local), so `A2aChanged` fires here.
        if let Ok(Some(actor)) = a2a(&self.rt) {
            let _ = actor.ask(a2a::contacts::ChatArchived { session }).await;
        }
        Ok(())
    }

    async fn create_session(
        &self,
        ext: &Extensions,
        options: CreateSessionOptions,
    ) -> RpcResult<SessionOpen> {
        let peer = peer(ext)?;
        // 183 §5.6 (W-10): New chat on another machine: that engine creates and
        // owns the row from epoch 1. Until the row replicates here, calls for it
        // are forwarded there by id (and its reads while it is in use). A
        // private tree belongs to this window's connection and never
        // replicates: it cannot start elsewhere.
        if let Some(engine) = options.engine.clone() {
            if options.retention == Retention::Private && !engine.is_empty() {
                let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
                if engine != store.engine() {
                    return Err(failed(
                        "a private chat runs on this machine; it cannot start on another",
                    ));
                }
            }
            let mut there = options.clone();
            there.engine = None;
            if let Some(open) =
                forward::to_engine::<SessionOpen>(ext, Some(&engine), "create_session", (there,))
                    .await?
            {
                sessionsync::owner::expect(open.session, replica::PeerId(engine.clone()));
                self.wait_for_row(open.session, &engine).await;
                return Ok(open);
            }
        }
        // A forwarded create names a folder of this machine (§13 row 12).
        if ext.get::<forward::Forwarded>().is_some()
            && let Some(cwd) = options.cwd.as_deref()
            && !Path::new(cwd).is_dir()
        {
            return Err(failed(format!(
                "{cwd} does not exist on {}",
                platform::process::machine().name
            )));
        }
        let open = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            CreateSession {
                options,
                peer: Some((peer.id(), peer.closed_token())),
            },
        )
        .await?;
        self.track(&open, peer.id());
        Ok(open)
    }

    async fn search(
        &self,
        query: String,
        categories: Option<Vec<SearchCategory>>,
        limit: Option<u32>,
    ) -> RpcResult<SearchResponse> {
        tracing::debug!(%query, ?categories, ?limit, "rpc search");
        crate::search::run(&self.rt, query, categories, limit)
            .await
            .map_err(|e| {
                ErrorObjectOwned::owned(jsonrpsee::types::error::INVALID_PARAMS_CODE, e, None::<()>)
            })
    }

    async fn search_refresh(&self, categories: Option<Vec<SearchCategory>>) -> RpcResult<()> {
        crate::search::refresh(categories)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e, None::<()>))
    }

    async fn resolve_approval(
        &self,
        ext: &Extensions,
        id: ApprovalId,
        decision: Decision,
    ) -> RpcResult<bool> {
        if let Some(r) = route_question(ext, &id.0, "resolve_approval", (id, decision)).await? {
            return Ok(r);
        }
        let pending = ask::<Approval, _>(tools::APPROVAL_ACTOR, Resolve { id, decision }).await?;
        // 130 E4: the receipt the companion reads back (`activity_list`).
        if pending {
            let what = match decision {
                Decision::Allow => "allowed",
                Decision::Deny => "denied",
            };
            tasks::activity::record(
                &self.rt,
                tasks::activity::ActivityKind::ApprovalResolved,
                &rpc_core::Client::actor(ext),
                &id.0.to_string(),
                format!("{what} approval {}", id.0),
            )
            .await;
        }
        Ok(pending)
    }

    async fn answer_ask(
        &self,
        ext: &Extensions,
        id: AskId,
        answers: Option<Vec<Answer>>,
    ) -> RpcResult<bool> {
        if let Some(r) = route_question(ext, &id.0, "answer_ask", (id, &answers)).await? {
            return Ok(r);
        }
        let what = if answers.is_some() {
            "answered"
        } else {
            "dismissed"
        };
        let pending = agent::answer_ask(id, answers)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>))?;
        if pending {
            tasks::activity::record(
                &self.rt,
                tasks::activity::ActivityKind::AskAnswered,
                &rpc_core::Client::actor(ext),
                &id.0.to_string(),
                format!("{what} question {}", id.0),
            )
            .await;
        }
        Ok(pending)
    }

    async fn get_approvals(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<ApprovalsState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 W-5: the effective value mixes the row's pin with the owner's
        // `tools.approvals`; owner offline → `-32010` (183-notes deviation 7).
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "get_approvals", (session,)).await?
        {
            return Ok(r);
        }
        // The row is the pin's truth (D4); the registry holds it only once the
        // session's actor spawned here, so a session idle since a restart or a
        // move would read "inherit". ponytail: a setter racing this read can be
        // overwritten by the row it replaced; both are user clicks.
        if let Some(s) = session
            && let Ok(store) = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await
            && let Ok(Some((pin, _))) = store.session_settings(s).await
        {
            let _ = ask::<ToolRegistry, _>(
                tools::REGISTRY_ACTOR,
                tools::RestoreApprovals { session: s, pin },
            )
            .await;
        }
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetApprovals { session }).await
    }

    async fn doc_resolve(
        &self,
        ext: &Extensions,
        binding: document::Binding,
    ) -> RpcResult<Option<document::DocId>> {
        let docs = documents()?;
        if let Some(id) = doc_send(
            &docs,
            document::Lookup {
                binding: binding.clone(),
            },
        )
        .await?
        {
            return Ok(Some(id));
        }
        let text = match &binding {
            // On demand (59 §7.1): a knowledge concept becomes a document from its current file.
            document::Binding::Knowledge(concept) => match send(
                &knowledge()?,
                Get {
                    id: concept.clone(),
                },
            )
            .await
            {
                Ok(c) => c.text,
                Err(e) if e.code() == ASK_FAILED => return Ok(None),
                Err(e) => return Err(e),
            },
            // An agent's charter becomes a document from the body of `agents/<name>.md`; theuth's is
            // `prompt.md`, read-only, never a document.
            document::Binding::AgentCharter(name) if name == roster::AgentName::DEFAULT => {
                return Ok(None);
            }
            document::Binding::AgentCharter(name) => {
                match send(&roster_actor()?, roster::GetAgent(name.clone())).await {
                    Ok(f) => f.body,
                    Err(e) if e.code() == ASK_FAILED => return Ok(None),
                    Err(e) => return Err(e),
                }
            }
            _ => return Ok(None),
        };
        let r = doc_send(
            &docs,
            document::Ensure {
                binding: binding.clone(),
                markdown: text,
            },
        )
        .await
        .map(Some);
        doc_forward(ext, "doc_resolve", (&binding,), r).await
    }

    async fn doc_export(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        format: artifact::ExportFormat,
        dest: Option<PathBuf>,
        assets: Vec<artifact::ExportAsset>,
    ) -> RpcResult<artifact::ExportResult> {
        let (peer, by) = principal(ext)?;
        if let Some(dest) = &dest {
            owner(ext)?;
            let home = dirs::home_dir().ok_or_else(|| failed("no home directory"))?;
            let inside = dest.is_absolute()
                && dest.starts_with(&home)
                && !dest
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir));
            if !inside {
                return Err(failed(format!(
                    "{}: not under {}",
                    dest.display(),
                    home.display()
                )));
            }
        }
        let docs = documents()?;
        let meta = doc_send(
            &docs,
            document::MetaOf {
                doc: doc.clone(),
                by,
            },
        )
        .await;
        member_outcome(&peer, &meta);
        let meta = meta?;
        let annotate = format == artifact::ExportFormat::Pdf;
        // 171 §4.4: the user's export carries each cell's last output and run line.
        let markdown = document::reader_markdown(&docs, doc, None, annotate)
            .await
            .map_err(|e| failed(e.to_string()))?;
        send(
            &artifact()?,
            artifact::ExportDoc {
                title: meta.title,
                format,
                markdown,
                dest,
                assets,
                record: None,
            },
        )
        .await
    }

    async fn doc_member_revoke(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        member: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::MemberRevoke {
                doc: doc.clone(),
                pub_: member.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_member_revoke", (&doc, &member), r).await?;
        // 59 §8.4: their live conns go too — here as well as on the hub (the guest may hang off either
        // engine's uplink). Link off (no conns) is not an error.
        if let Ok(link) = link()
            && let Ok(key) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&member)
            && let Ok(key) = <[u8; 32]>::try_from(key)
        {
            let _ = link.ask(link::DropGuest { key }).await;
        }
        Ok(())
    }

    async fn doc_link_create(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        role: document::Role,
        name: Option<String>,
        expires_ms: Option<i64>,
    ) -> RpcResult<document::LinkCreated> {
        owner(ext)?;
        // 184 §7.2: in a mesh the document's server makes the link; a lone engine (paired phones
        // or not) runs today's code unchanged.
        if mesh::has_peers() == Some(true) {
            return doc_link_create_in_mesh(&self.rt, ext, doc, role, name, expires_ms).await;
        }
        // The URL needs the relay host and an id devices dial (59 §8.2): Link must be on. 69: the mesh
        // device id when this engine has one — the hub, whichever engine it is, answers `/h/<mesh id>`
        // — else the engine id (a single engine before any device paired). Old engine-id links keep
        // resolving: the engine uplink stays registered.
        let status: LinkStatus = send(&link()?, link::Status).await?;
        if !status.enabled {
            return Err(failed(LINK_OFF));
        }
        let (ids, via) = link_ids(&status, None, false)?;
        let origin = link_origin(&status.relay);
        let r = doc_send(
            &documents()?,
            document::LinkCreate {
                doc: doc.clone(),
                role,
                name: name.clone(),
                expires_ms,
            },
        )
        .await
        .map(|(link, secret)| document::LinkCreated {
            url: format!("{origin}/d#{ids}.{doc}.{secret}"),
            link,
            via,
        });
        doc_forward(ext, "doc_link_create", (&doc, role, &name, expires_ms), r).await
    }

    async fn source_add(
        &self,
        ext: &Extensions,
        args: document::ingest::SourceAddArgs,
    ) -> RpcResult<document::Source> {
        owner(ext)?;
        // 16 §5 (2026-11-11): the authenticated owner may add a local-network URL.
        let r = nb_result(
            tools::egress::scope(
                true,
                document::ingest::source_add(
                    &documents()?,
                    args.clone(),
                    document::Principal::Owner { device: None },
                ),
            )
            .await,
        );
        doc_forward(ext, "source_add", (&args,), r).await
    }

    async fn source_refresh(
        &self,
        ext: &Extensions,
        id: document::DocId,
    ) -> RpcResult<document::Source> {
        owner(ext)?;
        // 16 §5 (2026-11-11): the authenticated owner refreshes under the owner's grant.
        let r = nb_result(
            tools::egress::scope(
                true,
                document::ingest::source_refresh(
                    &documents()?,
                    id.clone(),
                    document::Principal::Owner { device: None },
                ),
            )
            .await,
        );
        doc_forward(ext, "source_refresh", (&id,), r).await
    }

    async fn insight_skills(&self) -> RpcResult<Vec<String>> {
        let mut out = document::ingest::insights::builtin_names();
        out.extend(agent::notebook_context::insight_skills(
            &agent::skills::dir(&self.rt.data_dir),
        ));
        Ok(out)
    }

    async fn doc_move_home(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        to: String,
    ) -> RpcResult<document::DocSummary> {
        owner(ext)?;
        // §4.8: the note's server runs the checks and hands the live copy over.
        let docs = documents()?;
        if let Some(v) = to_doc_server(ext, &docs, &doc, "doc_move_home", (&doc, &to)).await? {
            return Ok(v);
        }
        let r = doc_send(
            &docs,
            document::MoveHome {
                doc,
                to: mesh::EngineId(to.clone()),
                checks: true,
            },
        )
        .await;
        // §12.4: the actor's copy names engine ids; the user reads machine names.
        match r {
            Err(e) if e.message().contains(&to) => {
                let st = match mesh_actor() {
                    Ok(m) => send(&m, mesh::Status).await.ok(),
                    Err(_) => None,
                };
                let name = st.as_ref().and_then(|st| machine_name(st, &to));
                Err(failed(named(e.message(), &to, name)))
            }
            r => r,
        }
    }

    async fn shutdown(&self, ext: &Extensions) -> RpcResult<()> {
        match ext.get::<rpc_core::RpcPeer>() {
            Some(p) => tracing::warn!(
                peer = p.id(),
                remote = p.remote(),
                "shutdown requested over rpc"
            ),
            None => tracing::warn!("shutdown requested over rpc (unauthenticated connection)"),
        }
        // 109 §9.2: the drain runs before the token (every turn's cancel is
        // its child); off the request so the caller's reply is immediate.
        let rt = self.rt.clone();
        tokio::spawn(async move {
            agent::supervisor::drain_engine(&rt, "shutdown").await;
            rt.shutdown.cancel();
        });
        Ok(())
    }

    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings> {
        let mut settings = send(&dictation()?, voice::CurrentSettings).await?;
        // The live slot (`set_polish_model`), not the one read at startup.
        settings.polish_model = ask::<Agents, _>(agent::AGENT_ACTOR, GetPolishModel)
            .await?
            .map(|m| m.id);
        Ok(settings)
    }

    async fn search_settings_get(&self) -> RpcResult<search_providers::SearchSettings> {
        let defaults = ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::GetDefaults).await?;
        let providers = ask::<search_providers::SearchProviders, _>(
            search_providers::SEARCH_PROVIDERS_ACTOR,
            search_providers::GetProviders,
        )
        .await?;
        Ok(search_providers::SearchSettings {
            providers,
            defaults,
        })
    }

    async fn search_settings_set(
        &self,
        settings: search_providers::SearchSettings,
    ) -> RpcResult<search_providers::SearchSettings> {
        let defaults = ask::<web::WebActor, _>(
            web::WEB_ACTOR,
            web::settings::SetDefaults(settings.defaults),
        )
        .await?;
        let providers = ask::<search_providers::SearchProviders, _>(
            search_providers::SEARCH_PROVIDERS_ACTOR,
            search_providers::SetProviders(settings.providers),
        )
        .await?;
        Ok(search_providers::SearchSettings {
            providers,
            defaults,
        })
    }

    async fn mesh_remove(&self, id: String, reset_devices: Option<bool>) -> RpcResult<()> {
        let reset_devices = reset_devices == Some(true);
        send(
            &mesh_actor()?,
            mesh::Unpair {
                id: mesh::EngineId(id.clone()),
                reset_devices,
            },
        )
        .await?;
        if !reset_devices {
            return Ok(());
        }
        // D11: the unpair's rotation stores the new vault key with `device_reset_pending`; the
        // device key follows, as a new `mesh/device_key` version sealed under that key, so it
        // replicates to the remaining machines only. A crash before here is retried by the
        // vault's own tick (183 R1).
        let failed = |what: String| {
            ErrorObjectOwned::owned(
                ASK_FAILED,
                format!("mesh_remove: unpaired, but phone and iPad pairing was not reset: {what}"),
                None::<()>,
            )
        };
        let vault = send(&secrets()?, secrets::Handle).await?;
        match vault.rotation() {
            Some(r) if r.phase != secrets::RotationPhase::Failed && r.removed.contains(&id) => {}
            r => {
                return Err(failed(
                    r.and_then(|r| r.error)
                        .unwrap_or_else(|| "the vault key did not rotate".into()),
                ));
            }
        }
        let reset = vault
            .finish_device_reset()
            .await
            .map_err(|e| failed(format!("{e:#}")))?;
        if reset && let Ok(l) = link() {
            send(&l, link::ResetDevices).await?;
        }
        Ok(())
    }

    async fn mesh_unpair_preview(&self, id: String) -> RpcResult<mesh::UnpairPreview> {
        let mut p = send(&mesh_actor()?, mesh::PreviewUnpair(mesh::EngineId(id))).await?;
        // W-37: the paired phones and iPads; none when link is not running here.
        if let Ok(l) = link() {
            let devices = send(&l, link::DevicesList).await?;
            p.devices = u32::try_from(devices.iter().filter(|d| !d.revoked).count()).unwrap_or(0);
        }
        Ok(p)
    }

    async fn task_move_home(
        &self,
        ext: &Extensions,
        id: String,
        to: String,
    ) -> RpcResult<tasks::Task> {
        let task = send(&tasks_actor()?, tasks::GetTask(tasks::TaskId(id.clone()))).await?;
        let actor = rpc_core::Client::actor(ext);
        let m = |check_only, session_moved| tasks::MoveHome {
            id: tasks::TaskId(id.clone()),
            to: to.clone(),
            actor: actor.clone(),
            session: None,
            check_only,
            session_moved,
        };
        // 182 S4: the move is checked before anything moves — a refused move
        // never leaves its session on the other machine.
        send(&tasks_actor()?, m(true, false)).await?;
        // 182 D10: a live work session of its checkout, owned here, moves first (181) so
        // it continues there; its refusal (a paused tree, the target refusing) refuses
        // the task's move. A desk or harness session stays: it is this engine's.
        let mut session_moved = false;
        if let Some(c) = task.checkout.as_ref().filter(|c| !c.orphaned)
            && let Ok(session) = c.session.parse::<SessionId>()
        {
            let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
            let me = store.engine().to_owned();
            let owned_here = to != me
                && matches!(store.fence(session).await, Ok(Some((_, owner))) if owner == me);
            let movable = owned_here
                && store
                    .session(session)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|s| s.desk.is_none() && s.agent.is_none());
            if movable {
                RpcSessionMoveExt::session_move(self, ext, session, to.clone()).await?;
                session_moved = true;
            }
        }
        send(&tasks_actor()?, m(false, session_moved)).await
    }

    async fn task_move_home_waiting(
        &self,
        ext: &Extensions,
        from: String,
    ) -> RpcResult<tasks::MovedHome> {
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let m = tasks::MoveWaiting {
            from,
            to: store.engine().to_owned(),
            actor: rpc_core::Client::actor(ext),
        };
        send(&tasks_actor()?, m).await
    }

    async fn get_app(
        &self,
        ext: &Extensions,
        record_call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<AppDoc> {
        let apps = apps()?;
        self.authorize_opt(ext, session).await?;
        // The app plugin reads any store by call id: the record must be one this caller may see.
        send(
            &artifact()?,
            artifact::search::Get {
                call_id: record_call_id.clone(),
                session,
            },
        )
        .await?;
        send(&apps, GetApp { record_call_id }).await
    }

    async fn subscribe_events(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult {
        // Subscribe before the snapshot and before accept so nothing published
        // in between is missed; a child in both the snapshot and the feed is
        // inserted twice, harmlessly.
        let mut rx = self.events.subscribe();
        // Admission first; a refusal is answered as the call's error (the
        // peer gate's `-32002`), not jsonrpsee's anonymous internal error.
        let admit = async {
            let peer = peer(ext)?.id();
            // Private trees opened elsewhere (another process's server, before
            // this map existed) become known here; the filter fails closed on
            // every member for everyone but its owner (51 §4.2).
            if let Ok(trees) = ask::<Agents, _>(agent::AGENT_ACTOR, PrivateTrees).await {
                self.private.write().extend(
                    trees
                        .into_iter()
                        .map(|(member, root, owner)| (member, (root, owner))),
                );
            }
            let tree: Option<HashSet<String>> = match (session, descendants) {
                (Some(root), Some(true)) => {
                    let mut set: HashSet<String> = self
                        .list_agents(ext, root)
                        .await?
                        .into_iter()
                        .map(|a| a.id.to_string())
                        .collect();
                    set.insert(root.to_string());
                    Some(set)
                }
                (Some(root), _) => {
                    // 183 §5.6: a chat just created elsewhere streams through the
                    // follow guard below (by id until its row lands).
                    self.authorize_fwd(ext, root).await?;
                    None
                }
                _ => None,
            };
            RpcResult::Ok((peer, tree))
        };
        let (peer, mut tree) = match admit.await {
            Ok(v) => v,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        // 68 §5: a session owned elsewhere streams its live events here while
        // this subscription lives (the guard drops with the task).
        let follow = session.and_then(sessionsync::events::follow);
        let session = session.map(|s| s.to_string());
        let private = self.private.clone();
        // The snapshot goes first, always (empty too): the client replaces its
        // pending set with it, so a card whose `*Resolved` was missed in a
        // relay gap is dropped and one whose `*Requested` was missed appears.
        let snapshot = PendingPrompts::collect(|v| {
            wanted(v, session.as_deref(), tree.as_mut()) && admitted(v, peer, &private)
        })
        .await;
        if let Ok(raw) = serde_json::value::to_raw_value(&snapshot)
            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
        {
            return Ok(());
        }
        // 126 §4.4: the chips and countdowns of pending cards follow the snapshot.
        if let Ok(Some(d)) = ActorRef::<double::actor::Double>::lookup(double::DOUBLE_ACTOR) {
            let _ = d.tell(double::actor::Republish).await;
        }
        // 129 §3.1: the ring is read by one task and the sink written by
        // another, joined by `Stream`: a client that stops reading never makes
        // `rx.recv()` fall behind; what waits merges (deltas) or queues
        // (`BACKLOG`, counted into `Lagged` on overflow).
        let state = Arc::new(parking_lot::Mutex::new(Stream {
            backlog: Backlog::default(),
            tree,
            closed: false,
        }));
        let notify = Arc::new(tokio::sync::Notify::new());
        let focus = self.focus.clone();
        let read = {
            let (state, notify, session, private) = (
                state.clone(),
                notify.clone(),
                session.clone(),
                private.clone(),
            );
            async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let pass = wanted(&ev, session.as_deref(), state.lock().tree.as_mut());
                            if !pass || !admitted(&ev, peer, &private) {
                                continue;
                            }
                            if is_delta(&ev) {
                                let focus = focus.read().get(&peer).cloned();
                                if let Some(set) = focus
                                    && !ev
                                        .get("session")
                                        .and_then(|s| s.as_str())
                                        .is_some_and(|s| set.contains(s))
                                {
                                    continue;
                                }
                            }
                            let now = Instant::now();
                            state.lock().backlog.push(ev, now);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            state.lock().backlog.lost += n
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            state.lock().closed = true;
                            notify.notify_one();
                            return;
                        }
                    }
                    notify.notify_one();
                }
            }
        };
        let write = async move {
            loop {
                let now = Instant::now();
                let next = state.lock().backlog.next(now);
                match next {
                    Next::Frame(ev) => {
                        if let Ok(raw) = serde_json::value::to_raw_value(&*ev)
                            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
                        {
                            return;
                        }
                    }
                    Next::Lagged(n) => {
                        // Dropped events may include a `*Resolved`: the snapshot after
                        // `Lagged` puts the client's pending set right again.
                        let lagged = serde_json::json!({ "type": "Lagged", "count": n });
                        if let Ok(raw) = serde_json::value::to_raw_value(&lagged)
                            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
                        {
                            return;
                        }
                        let snapshot = PendingPrompts::collect(|v| {
                            let pass = wanted(v, session.as_deref(), state.lock().tree.as_mut());
                            pass && admitted(v, peer, &private)
                        })
                        .await;
                        if let Ok(raw) = serde_json::value::to_raw_value(&snapshot)
                            && sink.send(SubscriptionMessage::from(raw)).await.is_err()
                        {
                            return;
                        }
                    }
                    Next::Wait(at) => {
                        let closed = state.lock().closed;
                        if closed {
                            return;
                        }
                        tokio::select! {
                            _ = sink.closed() => return,
                            _ = notify.notified() => {}
                            _ = tokio::time::sleep_until(at.unwrap_or(now)), if at.is_some() => {}
                        }
                    }
                }
            }
        };
        tokio::spawn(async move {
            let _follow = follow;
            let reader = tokio::spawn(read);
            write.await;
            reader.abort();
        });
        Ok(())
    }

    async fn set_event_focus(&self, ext: &Extensions, sessions: Vec<SessionId>) -> RpcResult<()> {
        let peer = peer(ext)?;
        let set: Arc<HashSet<String>> =
            Arc::new(sessions.iter().map(ToString::to_string).collect());
        // 183 §5.1: a focused session streams from its owner while focused. The
        // relay skips one owned here and follows it if it moves away; a session
        // leaving the focus drops its guard (the stream closes).
        {
            let mut follows = self.follows.lock();
            let held = follows.entry(peer.id()).or_default();
            held.retain(|s, _| sessions.contains(s));
            for s in &sessions {
                if !held.contains_key(s)
                    && let Some(guard) = sessionsync::events::follow(*s)
                {
                    held.insert(*s, guard);
                }
            }
        }
        let fresh = self.focus.write().insert(peer.id(), set.clone()).is_none();
        if fresh {
            self.forget_on_close(peer.clone());
        }
        // 155 §7.5: looking at a session marks its finished alert seen here.
        if !set.is_empty()
            && let Ok(actor) = attention_actor()
            && let Ok(mut items) = actor.ask(roster::attention::ListAttention).await
        {
            let mut seen = self.seen.write();
            let fresh = !seen.contains_key(&peer.id());
            mask_alerts(&mut items, Some(&set), seen.entry(peer.id()).or_default());
            drop(seen);
            if fresh {
                self.forget_on_close(peer);
            }
        }
        Ok(())
    }

    fn ping(&self) -> RpcResult<()> {
        Ok(())
    }

    fn client_log(
        &self,
        ext: &Extensions,
        level: String,
        source: String,
        message: String,
    ) -> RpcResult<()> {
        let conn = ext.get::<rpc_core::ConnId>().map_or(0, |c| c.0);
        crate::diag::client_log(&peer(ext)?, conn, &level, &source, &message);
        Ok(())
    }
}


