use super::*;

/// Where a `/model` / `/effort` pick goes (125 §7.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PickTarget {
    /// `set_model` / `set_effort`: the shown session, or the draft's pick before one exists (148 R1).
    #[default]
    Session,
    /// `set_advisor`: the shown session's advisor.
    Advisor,
    /// `set_harness_option` (75 §8, 194 D6): an ACP session's `model` select
    /// as the rows, its `thought_level` option as the chips (no Default).
    Harness,
}

impl Model {

    /// `/advisor [off|on|status|<provider/model>]` (125 §7.1): bare opens the
    /// `/model` picker targeted at the advisor; `on` re-uses the remembered
    /// model; anything else is a `provider/model` id. Not on an ACP child:
    /// the advisor reads theuth's transcript rows, which a harness does not produce.
    pub(super) fn advisor_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        if self.agent.is_some() {
            self.entries.push(Entry::Error(
                "/advisor: not available for an external harness session".into(),
            ));
            return;
        }
        if arg.is_empty() {
            self.pick_target = PickTarget::Advisor;
            send(tx, Command::ListModels);
            return;
        }
        // R1.2: the cap is an engine default, not a per-session setting.
        if let Some(n) = arg.strip_prefix("consults") {
            match n.trim().parse::<u8>() {
                Ok(n) if n <= 32 => send(tx, Command::SetAdvisorMaxConsults(n)),
                _ => self.entries.push(Entry::Error(format!(
                    "/advisor consults: expected a number 0-32, got {:?}",
                    n.trim()
                ))),
            }
            return;
        }
        let Some(session) = self.session else {
            self.entries
                .push(Entry::Error("no session yet: send a message first".into()));
            return;
        };
        let (model, effort) = match arg {
            "status" => {
                self.advisor_reply = Some(AdvisorReply::Status);
                send(tx, Command::AdvisorStatus(session));
                return;
            }
            "off" => (None, None),
            "on" => match self.advisor_default.model.clone() {
                Some(m) => (Some(m), self.advisor_default.effort.clone()),
                None => {
                    self.entries.push(Entry::Error(
                        "/advisor on: no remembered advisor model — pick one with /advisor".into(),
                    ));
                    return;
                }
            },
            id => (Some(id.to_owned()), None),
        };
        self.set_advisor(session, model, effort, tx);
    }

    /// `SetAdvisor` for `session`: a model pick or a chip; the reply installs silently.
    fn set_advisor(
        &mut self,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
        tx: &UnboundedSender<Command>,
    ) {
        self.advisor_reply = None;
        send(
            tx,
            Command::SetAdvisor {
                session,
                model,
                effort,
            },
        );
    }

    /// Install the shown session's advisor status (a reply or `AdvisorChanged`,
    /// whichever lands first): one `advisor → …` line when the model or the
    /// off state changed (125 §7.5), the D-R1.5 pairing warning once per link
    /// after it; a model remembers itself as the `/advisor on` default.
    pub(super) fn set_advisor_status(&mut self, status: AdvisorStatus) {
        let off = |s: &AdvisorStatus| s.state == AdvisorState::Off || s.model.is_none();
        let changed = status.model != self.advisor.model || off(&status) != off(&self.advisor);
        let warn = status.warning.is_some()
            && !off(&status)
            && (changed || status.warning != self.advisor.warning);
        if status.model.is_some() {
            self.advisor_default.model.clone_from(&status.model);
            self.advisor_default.effort.clone_from(&status.effort);
        }
        self.advisor = status;
        if changed {
            self.interject(Entry::Info(advisor_changed_line(&self.advisor)));
        }
        if let Some(w) = self.advisor.warning.as_deref().filter(|_| warn) {
            self.interject(Entry::Info(format!("advisor: {w}")));
        }
    }

    /// `/model <id>` or a picker pick (148 R1): `SetModel` for the shown
    /// session only; with none yet the draft's model, shown in the header.
    pub(super) fn pick_model(&mut self, model: String, tx: &UnboundedSender<Command>) {
        match self.session {
            Some(session) => send(tx, Command::SetModel { session, model }),
            None => {
                self.entries
                    .push(Entry::Info(format!("model → {model} (next session)")));
                self.model_id = model.clone();
                self.draft.get_or_insert_default().model = Some(model);
            }
        }
    }

    /// `/effort <v>` or the picker (148 R1): `SetEffort` for the shown session
    /// only (the reply announces it); with none yet the draft's effort
    /// (`None` = the engine default again).
    pub(super) fn set_effort(&mut self, effort: Option<String>, tx: &UnboundedSender<Command>) {
        match self.session {
            Some(session) => {
                self.awaiting_effort = true;
                send(tx, Command::SetEffort { session, effort });
            }
            None => {
                self.entries.push(Entry::Info(match &effort {
                    Some(e) => format!("effort: {e} (next session)"),
                    None => "effort cleared (next session)".to_owned(),
                }));
                self.effort = effort.clone().or_else(|| self.default_effort.clone());
                self.draft.get_or_insert_default().effort = effort;
            }
        }
    }

    /// The draft's options for a command that opens the session (148 R1):
    /// `Some` only while a pick is pending and no session is shown.
    pub(super) fn draft_options(&self) -> Option<CreateSessionOptions> {
        self.draft
            .clone()
            .filter(|d| self.session.is_none() && (d.model.is_some() || d.effort.is_some()))
    }
}

impl Model {

    /// `/model` picker rows (124 §3, 127 §3): the favorites present in the
    /// catalog, the picked model unless it is one of them, then one row per
    /// provider; a provider's models by name; or, with a filter, the picked
    /// model then the ranked results over every model (the picked one
    /// removed so it shows once). 194 D6: the picked model's effort chips
    /// directly under its row, wherever it shows.
    pub fn models_rows(&self) -> Vec<ModelRow<'_>> {
        let Some(Menu::Models {
            items,
            filter,
            provider,
            target,
            harness,
            ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let picked = self.pick_current();
        let current = items.iter().find(|m| Some(m.id.as_str()) == picked);
        // The harness picker stars nothing (as on the desktop).
        let favorites: &[String] = match target {
            PickTarget::Harness => &[],
            _ => &self.favorite_models,
        };
        let mut rows = Vec::new();
        // 125 §7.2: the advisor's picker leads with `Off` while browsing (either
        // level — a single provider skips the providers one), marked while there is none.
        if filter.is_empty() && *target == PickTarget::Advisor {
            rows.push(ModelRow::Off {
                current: picked.is_none(),
            });
        }
        if !filter.is_empty() {
            rows.extend(current.map(ModelRow::Current));
            rows.extend(
                rank_models(items, filter)
                    .into_iter()
                    .filter(|m| Some(m.id.as_str()) != picked)
                    .map(ModelRow::Model),
            );
        } else if let Some(p) = provider {
            rows.extend(provider_models(items, p).into_iter().map(ModelRow::Model));
        } else {
            rows.extend(
                favorites
                    .iter()
                    .filter_map(|f| items.iter().find(|m| m.id == *f))
                    .map(ModelRow::Favorite),
            );
            if !picked.is_some_and(|p| favorites.iter().any(|f| f == p)) {
                rows.extend(current.map(ModelRow::Current));
            }
            let now = current.map(|c| c.provider.as_str());
            rows.extend(providers(items).into_iter().map(|(id, count)| {
                let current = now == Some(id.as_str());
                ModelRow::Provider { id, count, current }
            }));
        }
        if let Some(chips) = current.and_then(|c| self.effort_chips(*target, &c.efforts, harness))
            && let Some(at) = rows
                .iter()
                .position(|r| r.model().is_some_and(|m| Some(m.id.as_str()) == picked))
        {
            rows.insert(at + 1, ModelRow::Efforts(chips));
        }
        rows
    }

    /// 194 D6: the chip row over the picked model's `levels` — Default first
    /// unless the target requires a level (a harness), the stored effort active.
    fn effort_chips(
        &self,
        target: PickTarget,
        levels: &[String],
        harness: &[acp::ConfigOption],
    ) -> Option<EffortChips> {
        if levels.is_empty() {
            return None;
        }
        let (required, current) = match target {
            PickTarget::Session if self.session.is_some() => (false, self.effort.as_deref()),
            // 148 R1: before a session the draft's pick; none is the engine default.
            PickTarget::Session => (false, self.draft.as_ref().and_then(|d| d.effort.as_deref())),
            PickTarget::Advisor => (false, self.advisor.effort.as_deref()),
            PickTarget::Harness => (true, harness_groups(harness).1.map(|o| o.current.as_str())),
        };
        let levels: Vec<Option<String>> = (!required)
            .then_some(None)
            .into_iter()
            .chain(levels.iter().cloned().map(Some))
            .collect();
        let active = levels.iter().position(|l| l.as_deref() == current);
        Some(EffortChips { levels, active })
    }

    /// The model the open `/model` picker marks `●`: the session's, the
    /// advisor's (`None` while off) when the picker targets it (125 §7.2), or
    /// the harness's `model` value (its one row without a model select).
    pub fn pick_current(&self) -> Option<&str> {
        match &self.menu {
            Some(Menu::Models {
                target: PickTarget::Advisor,
                ..
            }) => self.advisor.model.as_deref(),
            Some(Menu::Models {
                target: PickTarget::Harness,
                items,
                harness,
                ..
            }) => harness_groups(harness)
                .0
                .map(|o| o.current.as_str())
                .or_else(|| items.first().map(|m| m.id.as_str())),
            _ => Some(self.model_id.as_str()),
        }
    }

    /// Apply the engine's favorite list (127 §3); an open `/model` picker's
    /// cursor is clamped to the rows it now has.
    pub(super) fn set_favorites(&mut self, models: Vec<String>) {
        self.favorite_models = models;
        let len = self.models_rows().len();
        if let Some(Menu::Models { cursor, .. }) = &mut self.menu {
            *cursor = (*cursor).min(len.saturating_sub(1));
        }
    }

    /// Open the picker over `items` for `target` (124 §3.5: one provider skips
    /// the providers level); `chips` (bare `/effort`) starts on the chip row.
    pub(super) fn open_models(
        &mut self,
        items: Vec<ModelInfo>,
        target: PickTarget,
        harness: Vec<acp::ConfigOption>,
        chips: bool,
    ) {
        let provider = match providers(&items).as_slice() {
            [(p, _)] => Some(p.clone()),
            _ => None,
        };
        self.menu = Some(Menu::Models {
            items,
            filter: String::new(),
            cursor: 0,
            chip: 0,
            provider,
            target,
            harness,
        });
        self.models_home();
        let at = self
            .models_rows()
            .iter()
            .enumerate()
            .find_map(|(i, r)| match r {
                ModelRow::Efforts(c) if chips => Some((i, c.active.unwrap_or(0))),
                _ => None,
            });
        if let (Some((row, active)), Some(Menu::Models { cursor, chip, .. })) = (at, &mut self.menu)
        {
            *cursor = row;
            *chip = active;
        }
    }

    /// Put the `/model` cursor where a level starts: the first row after the
    /// Off and Current rows and their chips (first favorite, first provider,
    /// or best search hit) when there is one.
    pub(super) fn models_home(&mut self) {
        let rows = self.models_rows();
        let lead = rows
            .iter()
            .take_while(|r| {
                matches!(
                    r,
                    ModelRow::Off { .. } | ModelRow::Current(_) | ModelRow::Efforts(_)
                )
            })
            .count();
        let home = if rows.len() > lead { lead } else { 0 };
        if let Some(Menu::Models { cursor, .. }) = &mut self.menu {
            *cursor = home;
        }
    }
}

impl Model {

    /// `/model` (124 §3, 127 §3): ↑↓ move; typing searches every model; ⏎/→
    /// on a provider opens it; ←, or ⌫ on an empty filter, returns to the
    /// providers level (no-op with one provider); ⏎ on a model sets it; ⇥ on
    /// a model stars or unstars it (the menu stays open; not on a harness);
    /// 194 D6: on the chip row ←→ move between chips and ⏎ sets that effort
    /// (Default clears it); Esc closes.
    pub(super) fn models_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        /// What the highlighted row is.
        enum Hit {
            Provider(String),
            /// A model row: its id and effort levels.
            Model(String, Vec<String>),
            /// The advisor's `Off` row.
            Off,
            /// The chip row's chips (`None` = Default).
            Chips(Vec<Option<String>>),
        }
        let Some(Menu::Models {
            items,
            cursor,
            target,
            harness,
            ..
        }) = &self.menu
        else {
            return MenuKey::Consumed;
        };
        let target = *target;
        let (model_option, effort_option) = {
            let (m, e) = harness_groups(harness);
            (m.map(|o| o.id.clone()), e.map(|o| o.id.clone()))
        };
        let many = providers(items).len() > 1;
        let rows = self.models_rows();
        let len = rows.len();
        let at = (*cursor).min(len.saturating_sub(1));
        let hit = match rows.get(at) {
            Some(ModelRow::Current(m) | ModelRow::Favorite(m) | ModelRow::Model(m)) => {
                Some(Hit::Model(m.id.clone(), m.efforts.clone()))
            }
            Some(ModelRow::Provider { id, .. }) => Some(Hit::Provider(id.clone())),
            Some(ModelRow::Off { .. }) => Some(Hit::Off),
            Some(ModelRow::Efforts(c)) => Some(Hit::Chips(c.levels.clone())),
            None => None,
        };
        // ↑↓ onto the chip row land on its active chip.
        let chips_at = rows.iter().enumerate().find_map(|(i, r)| match r {
            ModelRow::Efforts(c) => Some((i, c.active.unwrap_or(0))),
            _ => None,
        });
        let Some(Menu::Models {
            filter,
            cursor,
            chip,
            provider,
            ..
        }) = &mut self.menu
        else {
            return MenuKey::Consumed;
        };
        *cursor = at;
        let mut home = false;
        match code {
            KeyCode::Up | KeyCode::Down => {
                *cursor = match code {
                    KeyCode::Up => cursor.saturating_sub(1),
                    _ => (*cursor + 1).min(len.saturating_sub(1)),
                };
                if let Some((row, active)) = chips_at
                    && row == *cursor
                {
                    *chip = active;
                }
            }
            KeyCode::Char(c) => {
                filter.push(c);
                home = true;
            }
            KeyCode::Backspace => {
                if filter.pop().is_some() {
                    home = true;
                } else if many && provider.is_some() {
                    *provider = None;
                    home = true;
                }
            }
            KeyCode::Left => match hit {
                Some(Hit::Chips(_)) => *chip = chip.saturating_sub(1),
                _ if filter.is_empty() && many && provider.is_some() => {
                    *provider = None;
                    home = true;
                }
                _ => {}
            },
            KeyCode::Right => match hit {
                Some(Hit::Chips(levels)) => *chip = (*chip + 1).min(levels.len().saturating_sub(1)),
                Some(Hit::Provider(p)) => {
                    *provider = Some(p);
                    home = true;
                }
                _ => {}
            },
            KeyCode::Enter => match hit {
                Some(Hit::Provider(p)) => {
                    *provider = Some(p);
                    home = true;
                }
                Some(Hit::Chips(levels)) => {
                    let level = levels.get(*chip).cloned();
                    self.menu = None;
                    if let Some(level) = level {
                        self.pick_chip(target, level, effort_option, tx);
                    }
                }
                Some(Hit::Model(model, efforts)) => {
                    self.menu = None;
                    self.pick_row(target, Some(model), &efforts, model_option, tx);
                }
                Some(Hit::Off) => {
                    self.menu = None;
                    self.pick_row(target, None, &[], model_option, tx);
                }
                None => {}
            },
            KeyCode::Esc => self.menu = None,
            KeyCode::Tab => {
                if let Some(Hit::Model(model, _)) = hit
                    && target != PickTarget::Harness
                {
                    let favorite = !self.favorite_models.contains(&model);
                    send(tx, Command::SetModelFavorite { model, favorite });
                }
            }
            _ => {}
        }
        if home {
            self.models_home();
        }
        MenuKey::Consumed
    }

    /// ⏎ on a model row (`None`: the advisor's `Off`): the target's model
    /// setter. `efforts` are the picked model's levels; `option` is the
    /// harness's `model` select.
    fn pick_row(
        &mut self,
        target: PickTarget,
        model: Option<String>,
        efforts: &[String],
        option: Option<String>,
        tx: &UnboundedSender<Command>,
    ) {
        match target {
            PickTarget::Session => {
                if let Some(model) = model {
                    self.pick_model(model, tx);
                }
            }
            // 125 §7.2: a model or `Off` goes to `set_advisor`; a model keeps
            // the advisor's effort when it takes it (194 D4c), else Default.
            PickTarget::Advisor => match self.session {
                Some(s) => {
                    let effort = self
                        .advisor
                        .effort
                        .clone()
                        .filter(|e| model.is_some() && efforts.contains(e));
                    self.set_advisor(s, model, effort, tx);
                }
                None => self
                    .entries
                    .push(Entry::Error("no session yet: send a message first".into())),
            },
            // A harness without a model select: its one row only closes the picker.
            PickTarget::Harness => {
                if let (Some(session), Some(config_id), Some(value)) = (self.session, option, model)
                {
                    send(
                        tx,
                        Command::SetHarnessOption {
                            session,
                            config_id,
                            value,
                        },
                    );
                }
            }
        }
    }

    /// ⏎ on a chip (194 D6): the target's effort setter, as `/effort` does;
    /// `None` is the Default chip. `option` is the harness's `thought_level` one.
    fn pick_chip(
        &mut self,
        target: PickTarget,
        effort: Option<String>,
        option: Option<String>,
        tx: &UnboundedSender<Command>,
    ) {
        match target {
            PickTarget::Session => self.set_effort(effort, tx),
            PickTarget::Advisor => {
                // The advisor went off while the picker was open: nothing to set.
                if let (Some(session), model @ Some(_)) = (self.session, self.advisor.model.clone())
                {
                    self.set_advisor(session, model, effort, tx);
                }
            }
            PickTarget::Harness => {
                if let (Some(session), Some(config_id), Some(value)) =
                    (self.session, option, effort)
                {
                    send(
                        tx,
                        Command::SetHarnessOption {
                            session,
                            config_id,
                            value,
                        },
                    );
                }
            }
        }
    }

    /// `/aws`: Enter selects the highlighted profile for the whole harness;
    /// row 0 restores the AWS default credential chain.
    pub(super) fn aws_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::AwsProfiles { info, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(info.profiles.len()),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = cursor.checked_sub(1).map(|i| info.profiles.get(i).cloned());
                self.menu = None;
                match picked {
                    None => send(tx, Command::SetAwsProfile(None)),
                    Some(Some(p)) => send(tx, Command::SetAwsProfile(Some(p))),
                    Some(None) => {}
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// `/aws` picker rows: the default chain, then every SDK-discovered
    /// profile; `●` marks the selection in force.
    pub fn aws_rows(&self) -> Vec<String> {
        let Some(Menu::AwsProfiles { info, .. }) = &self.menu else {
            return Vec::new();
        };
        let mark = |on: bool| if on { "●" } else { " " };
        std::iter::once(format!(
            "{} default credential chain ({})",
            mark(info.selected.is_none()),
            info.default_profile
        ))
        .chain(
            info.profiles
                .iter()
                .map(|p| format!("{} {p}", mark(info.selected.as_deref() == Some(p.as_str())))),
        )
        .collect()
    }
}

/// A `/model` picker row (124 §3).
#[derive(Debug)]
pub enum ModelRow<'a> {
    /// 125 §7.2: the advisor picker's lead row; `current` while no advisor is set.
    Off {
        /// Whether the advisor is off now.
        current: bool,
    },
    /// The session's current model: always first, never filtered out.
    Current(&'a ModelInfo),
    /// A provider on the providers level; `current` marks the current model's.
    Provider {
        /// Provider id as it appears in `provider/model`.
        id: String,
        /// Models listed under it.
        count: usize,
        /// Whether the current model belongs to it.
        current: bool,
    },
    /// A starred model on the providers level (127 §3), in favorites order.
    Favorite(&'a ModelInfo),
    /// A model on a provider's level or in the search results.
    Model(&'a ModelInfo),
    /// 194 D6: the picked model's effort chips, directly under its row.
    Efforts(EffortChips),
}

impl ModelRow<'_> {
    /// The model a Current, Favorite or Model row stands for.
    pub fn model(&self) -> Option<&ModelInfo> {
        match self {
            Self::Current(m) | Self::Favorite(m) | Self::Model(m) => Some(m),
            _ => None,
        }
    }
}

/// The chip row under the picked model (194 D6).
#[derive(Debug, PartialEq, Eq)]
pub struct EffortChips {
    /// The chips in order: `None` is Default (absent when a level is
    /// required), then the levels.
    pub levels: Vec<Option<String>>,
    /// The active chip; `None` when the stored effort is not one of them.
    pub active: Option<usize>,
}

/// Search terms (124 §2.1): lowercased words of `query`; none means browse mode.
pub(super) fn search_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether `t` occurs in `s` at the start of a word (124 §2.2): at index 0 or
/// after a character outside `[a-z0-9]`.
fn boundary_match(s: &str, t: &str) -> bool {
    s.match_indices(t)
        .any(|(i, _)| !s[..i].ends_with(|c: char| c.is_ascii_alphanumeric()))
}

/// Tier of one search term for one model (124 §2.2 table), before gating.
fn term_tier(name: &str, id: &str, provider: &str, t: &str) -> u32 {
    let tail = id.split_once('/').map_or(id, |(_, tail)| tail);
    if name == t || tail == t {
        100
    } else if name.starts_with(t) {
        90
    } else if boundary_match(name, t) {
        80
    } else if provider == t {
        70
    } else if provider.starts_with(t) {
        65
    } else if boundary_match(tail, t) {
        60
    } else if name.contains(t) {
        40
    } else if id.contains(t) {
        30
    } else {
        0
    }
}

/// Word search over `items` (124 §2): every term must match; per term, plain
/// substring hits (40/30) only count when nothing matches at a word boundary;
/// score is the sum of tiers, ties keep catalog order. Browse mode (no terms)
/// returns everything.
pub fn rank_models<'a>(items: &'a [ModelInfo], query: &str) -> Vec<&'a ModelInfo> {
    let lower: Vec<(String, String, String)> = items
        .iter()
        .map(|m| {
            (
                m.name.to_lowercase(),
                m.id.to_lowercase(),
                m.provider.to_lowercase(),
            )
        })
        .collect();
    let mut scores = vec![Some(0u32); items.len()];
    for t in search_terms(query) {
        let tiers: Vec<u32> = lower
            .iter()
            .map(|(name, id, provider)| term_tier(name, id, provider, &t))
            .collect();
        let gate = tiers.iter().any(|&tier| tier >= 60);
        for (score, &tier) in scores.iter_mut().zip(&tiers) {
            match tier {
                0 => *score = None,
                1..60 if gate => *score = None,
                _ => {
                    if let Some(s) = score {
                        *s += tier;
                    }
                }
            }
        }
    }
    let mut hits: Vec<(u32, &ModelInfo)> = scores
        .iter()
        .zip(items)
        .filter_map(|(score, m)| score.map(|s| (s, m)))
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter().map(|(_, m)| m).collect()
}

/// Distinct providers of `items` with their model counts, A→Z (124 §3.1).
pub fn providers(items: &[ModelInfo]) -> Vec<(String, usize)> {
    let mut counts = std::collections::BTreeMap::new();
    for m in items {
        *counts.entry(m.provider.as_str()).or_insert(0) += 1;
    }
    counts.into_iter().map(|(p, n)| (p.to_owned(), n)).collect()
}

/// The models of `provider`, by name (case-insensitive) then id (124 §3.2).
pub fn provider_models<'a>(items: &'a [ModelInfo], provider: &str) -> Vec<&'a ModelInfo> {
    let mut out: Vec<&ModelInfo> = items.iter().filter(|m| m.provider == provider).collect();
    out.sort_by_cached_key(|m| (m.name.to_lowercase(), m.id.as_str()));
    out
}

/// The `/title` reply: the model naming sessions, or the fallback.
pub fn title_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!("title model: {} ({})", m.id, m.name),
        None => "title model: off — each session names itself with its own model (/title <model> to set a cheap one)".into(),
    }
}

/// The `/vision` reply: the model that inspects images for a text-only main
/// model (52 §3), or off.
pub fn vision_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!(
            "vision fallback: {} ({}) — used only when the main model cannot view images; it receives the image and a question written by the main model, an extra call to that provider",
            m.id, m.name
        ),
        None => "vision fallback: off — a text-only main model gets images as omitted (/vision <model> to set one)".into(),
    }
}

/// The `/aws` reply: which AWS profile Bedrock calls use, harness-wide.
pub fn aws_profile_line(p: &AwsProfiles) -> String {
    let now = match &p.selected {
        Some(s) if p.profiles.contains(s) => s.clone(),
        Some(s) => {
            format!("{s} (not found by the AWS SDK; `/aws clear` restores the default chain)")
        }
        None => format!(
            "default credential chain ({}), unless a Bedrock API key is configured",
            p.default_profile
        ),
    };
    let found = if p.profiles.is_empty() {
        "none found".to_owned()
    } else {
        p.profiles.join(", ")
    };
    format!(
        "aws profile: {now} — applies to every session's Bedrock calls from the next request\n  profiles: {found}"
    )
}
