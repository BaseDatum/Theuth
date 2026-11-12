use super::*;

/// Unix ms now.
pub(crate) fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// How full the context window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Under 75 %.
    Normal,
    /// 75–90 %: context filling up.
    Warn,
    /// 90 % and above: start a new session or summarise.
    Danger,
}

/// `12.3k`, `200k`, `1.0M`.
pub fn k(n: u32) -> String {
    let k = f64::from(n) / 1000.0;
    if n < 1000 {
        n.to_string()
    } else if k < 100.0 {
        format!("{k:.1}k")
    } else if k < 1000.0 {
        format!("{k:.0}k")
    } else {
        format!("{:.1}M", k / 1000.0)
    }
}

impl Model {

    /// Some assistant text is still typing out.
    pub fn revealing(&self) -> bool {
        self.entries
            .iter()
            .rev()
            .any(|e| matches!(e, Entry::Assistant(t) if t.unrevealed() > 0))
    }

    /// Frames are needed: text is typing out, a turn is in flight, MCP
    /// servers are connecting, a compaction runs, or a subagent is live (the
    /// board's spinner and elapsed column).
    pub fn animating(&self) -> bool {
        self.revealing()
            || self.streaming
            || self.mcp_connecting()
            || self.compacting
            || self.harvesting
            || self.vision.is_some()
            || self.advisor.state == AdvisorState::Reviewing
            || self.agents.values().any(AgentRow::live)
            || self.components.wake().is_some()
            || self.double_counting_down()
            || self.invite.is_some()
            || self.dictation.is_some()
            || self.talk_view
    }
}

impl Model {

    /// What the shown session is doing right now, for the status line and
    /// the trailing activity row; `None` when idle. A background harvest is
    /// named only when the session is otherwise idle.
    pub fn activity(&self) -> Option<String> {
        if self.compacting {
            return Some("compacting…".into());
        }
        if let Some(v) = &self.vision {
            return Some(v.clone());
        }
        if !self.streaming {
            return self.harvesting.then(|| "harvesting…".into());
        }
        // Parallel calls: a running card need not be the last entry.
        let busy = self.entries.iter().rev().find_map(|e| match e {
            Entry::Tool {
                name,
                result: None,
                scrape,
                ..
            } => Some(scrape.clone().unwrap_or_else(|| format!("running {name}…"))),
            Entry::Pending {
                name,
                progress: Some(p),
                ..
            } => Some(progress_line(name, p)),
            Entry::Pending { name, .. } => Some(format!("preparing {name}…")),
            _ => None,
        });
        if busy.is_some() {
            return busy;
        }
        Some(match self.entries.last() {
            Some(Entry::Assistant(t)) if !t.is_done() => "streaming…".into(),
            Some(Entry::Reasoning { done: false, .. }) => "reasoning…".into(),
            _ => "thinking…".into(),
        })
    }

    /// The transcript's trailing activity row: only while waiting on the
    /// model with nothing else showing progress in place.
    pub fn waiting(&self) -> bool {
        self.activity().is_some_and(|a| a == "thinking…")
    }

    /// Current spinner glyph.
    pub fn spinner(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[(self.frame / 5) % FRAMES.len()]
    }

    /// One animation frame: advance the spinner and reveal a slice of the
    /// typewriter backlog proportional to its size, so the cursor trails the
    /// provider by a fixed number of frames (~a quarter second at 60 Hz)
    /// however fast it streams, and settles smoothly when it stops.
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        for e in self.entries.iter_mut().rev() {
            if let Entry::Assistant(t) = e {
                let backlog = t.unrevealed();
                if backlog == 0 {
                    continue;
                }
                t.reveal((backlog / 16).max(1));
            }
        }
        tick_card(&mut self.entries, &mut self.invite, CardKind::Node);
    }
}

impl Model {

    /// One-line status: session, streaming state, last usage, idle
    /// subagents (while the board is hidden; it has the live ones), queue.
    pub fn status(&self) -> String {
        let session = self.session.map_or_else(|| "no session".to_owned(), short);
        let state = self
            .activity()
            .map_or_else(|| "idle".to_owned(), |a| format!("{} {a}", self.spinner()));
        let usage = self.usage.map_or_else(String::new, |u| {
            format!("  {}↑ {}↓", u.prompt, u.completion)
        });
        let agents = match self.agents.len() {
            0 => String::new(),
            _ if self.agents.values().any(AgentRow::on_board) => String::new(),
            n => format!("  agents: {n} idle"),
        };
        let queued = match self.queued.len() {
            0 => String::new(),
            n => format!("  ·  {n} queued"),
        };
        let harness_skills: Vec<&str> = self
            .harness_commands
            .iter()
            .filter_map(|c| c.name.strip_prefix("skill:"))
            .collect();
        let skills = match (self.harness_commands.len(), self.loaded_skills.len()) {
            (0, 0) => String::new(),
            (0, _) => format!("  ·  skills: {}", self.loaded_skills.join(", ")),
            (n, _) if harness_skills.is_empty() => format!("  ·  harness commands: {n}"),
            _ => format!("  ·  harness skills: {}", harness_skills.join(", ")),
        };
        let scrolled = if self.anchor.is_some() {
            "  ·  ↑ history · PgDn follows"
        } else {
            ""
        };
        let editing = self.editing.map_or_else(String::new, |(idx, _)| {
            let n = self.entries[..idx.min(self.entries.len())]
                .iter()
                .filter(|e| matches!(e, Entry::User { turn: Some(_), .. }))
                .count()
                + 1;
            format!("  ·  editing #{n} — ⏎ sends, esc cancels")
        });
        let todos = if self.todo_focused() {
            "  ·  todos: ↑↓ scroll · esc closes · tab editor"
        } else {
            ""
        };
        let talk = match &self.talk {
            Some(t) if !self.talk_view => {
                let state = format!("{:?}", t.state).to_lowercase();
                format!("  ·  ● talk: {state} (/talk)")
            }
            _ => String::new(),
        };
        let attached = self
            .attached
            .as_deref()
            .map_or_else(String::new, |a| format!("  ·  {a}"));
        let resumed = self
            .resume_note
            .as_deref()
            .map_or_else(String::new, |n| format!("  ·  {n}"));
        let watch = self
            .watch_hint()
            .map_or_else(String::new, |h| format!("  ·  {h}"));
        use crate::components::{StatusSlot, Surface};
        let work = self.component_text(Surface::Status(StatusSlot::Work));
        let catalog = self.component_text(Surface::Status(StatusSlot::Catalog));
        let pinned = self.component_text(Surface::Status(StatusSlot::Pinned));
        let spoken = self.component_text(Surface::Status(StatusSlot::Spoken));
        format!(
            "{session}  {state}{watch}{usage}{agents}{queued}{work}{skills}{catalog}{editing}{scrolled}{todos}{pinned}{talk}{spoken}{attached}{resumed}"
        )
    }

    /// Line above the input: `model  ·  high  ·  ctx 12.3k / 200k (6%)  ·  cache 94%`
    /// and its colour level. In a subagent the model is headed by its peer
    /// name: `↳ grep-logs  child · model  ·  …`. An ACP child shows the
    /// harness and its reported model: `omp · anthropic/claude-opus-4-8`
    /// (06 §8.2). The effort segment is omitted while unset; `cache` is the
    /// prefix-cache hit ratio of the last finished turn (design doc 07 §7),
    /// omitted until one has reported usage.
    pub fn header(&self, catalog: &Catalog) -> (String, Level) {
        // 141 R3 T14 (Q3): a watch-only omp child's meter is the harness's
        // own `contextTokens/contextWindow` when it reported them.
        let (context, estimated, own_limit) = match self
            .observed
            .as_ref()
            .and_then(|o| o.detail["contextTokens"].as_u64())
        {
            Some(t) => (
                u32::try_from(t).unwrap_or(u32::MAX),
                false,
                self.observed
                    .as_ref()
                    .and_then(|o| o.detail["contextWindow"].as_u64())
                    .and_then(|w| u32::try_from(w).ok()),
            ),
            None => (self.context, self.context_estimated, None),
        };
        let used = format!("{}{}", if estimated { "≈" } else { "" }, k(context));
        let cache = match self.usage {
            Some(u) if u.prompt > 0 => {
                format!(
                    "  ·  cache {}%",
                    u64::from(u.cached) * 100 / u64::from(u.prompt)
                )
            }
            _ => String::new(),
        };
        let effort = self
            .effort
            .as_deref()
            .map_or_else(String::new, |e| format!("  ·  {e}"));
        // 141 §10.1: a watch-only child shows its own model when the harness
        // reported one, else the parent's (what `Loaded.model` carries).
        let model = match (&self.observed, &self.harness_model) {
            (Some(_), Some(m)) => m.clone(),
            (None, Some(m)) => format!("{} · {m}", self.model_id),
            (_, None) => self.model_id.clone(),
        };
        let model = match (&self.observed, &self.session_name) {
            (Some(_), name) => format!(
                "{WATCH_GLYPH} {}  watching · {model}",
                name.as_deref().unwrap_or("subagent")
            ),
            (None, Some(name)) => format!("↳ {name}  child · {model}"),
            (None, None) => model,
        };
        let limit = own_limit
            .or(self.context_size)
            .or_else(|| catalog.closest(&self.model_id).map(|m| m.limit.context));
        match limit {
            Some(limit) if limit > 0 => {
                let pct = u64::from(context) * 100 / u64::from(limit);
                let level = match pct {
                    0..75 => Level::Normal,
                    75..90 => Level::Warn,
                    _ => Level::Danger,
                };
                (
                    format!(
                        "{model}{effort}  ·  ctx {used} / {} ({pct}%){cache}{}",
                        k(limit),
                        self.header_suffix()
                    ),
                    level,
                )
            }
            _ => (
                format!(
                    "{model}{effort}  ·  ctx {used}{cache}{}",
                    self.header_suffix()
                ),
                Level::Normal,
            ),
        }
    }

    fn header_suffix(&self) -> String {
        use crate::components::{HeaderSlot, Surface};
        let mut text = String::new();
        for slot in HeaderSlot::ALL {
            match slot {
                HeaderSlot::Costs => text.push_str(&self.cost_tag()),
                HeaderSlot::Advisor => text.push_str(&self.advisor_tag()),
                HeaderSlot::Approvals => text.push_str(self.approvals_tag()),
                HeaderSlot::Communication => text.push_str(&self.communication_tag()),
                HeaderSlot::Link => {}
                HeaderSlot::Nodes => text.push_str(&self.nodes_tag()),
                HeaderSlot::Update => {}
            }
            text.push_str(&self.component_text(Surface::Header(slot)));
        }
        text
    }

    /// `  ·  advisor $0.012` (125 §7.5): the advisor session's spend from the
    /// last `AdvisorStatus`; empty while off or before it was priced.
    fn advisor_tag(&self) -> String {
        match (&self.advisor.state, &self.advisor.cost) {
            (AdvisorState::Off, _) | (_, None) => String::new(),
            (_, Some(c)) => format!("  ·  advisor {}", cost_short(c)),
        }
    }

    /// The status line's advisor segment (125 §7.5): `  ·  ◎ advisor: idle` /
    /// `⠋ reviewing…` / `paused` / `halted` with the state that colours it;
    /// `None` while off.
    pub fn advisor_segment(&self) -> Option<(String, AdvisorState)> {
        let state = self.advisor.state;
        let word = match state {
            AdvisorState::Off => return None,
            AdvisorState::Reviewing => format!("{} reviewing…", self.spinner()),
            s => advisor_state_name(s).to_owned(),
        };
        Some((format!("  ·  {ADVISOR_GLYPH} advisor: {word}"), state))
    }

    fn approvals_tag(&self) -> &'static str {
        if !self.approvals_global || self.approvals_off {
            "  ·  approvals off"
        } else {
            ""
        }
    }

    /// `  ·  communication off` while the authoritative state says the
    /// shown session (or the global default) is not effectively enabled;
    /// `  ·  communication on` when it is; empty until fetched. Never
    /// inferred from a local toggle (50 §0.8).
    fn communication_tag(&self) -> &'static str {
        match &self.communication {
            None => "",
            Some(c) if c.effective => "  ·  communication on",
            Some(_) => "  ·  communication off",
        }
    }


    /// `  ·  nodes ●N` (N = nodes online) while a node is paired; empty otherwise.
    fn nodes_tag(&self) -> String {
        match self.nodes.as_deref() {
            Some([]) | None => String::new(),
            Some(list) => format!("  ·  nodes ●{}", list.iter().filter(|n| n.online).count()),
        }
    }

}


/// Running time: `48s`, `1m32s`, `2h05m`.
pub(crate) fn elapsed(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m{s:02}s"),
        (h, m, _) => format!("{h}h{m:02}m"),
    }
}

/// An RFC 3339 stamp as local `YYYY-MM-DD HH:MM`; anything else verbatim.
pub(crate) fn when(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339).map_or_else(
        |_| rfc3339.to_owned(),
        |t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

/// Time since a turn ended, coarse: `48s ago`, `3m ago`, `2h ago`, `3d ago`.
pub(crate) fn ago(d: chrono::Duration) -> String {
    format!("{} ago", crate::broadcast::since(d))
}
