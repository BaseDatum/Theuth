//! Advisor selection, linked-session chrome and advisory transcript cards.
use super::*;
use std::collections::{BTreeSet, HashMap};
use agent::{AdvisorDefaults, AdvisorSeverity, AdvisorState, AdvisorStatus, Advisory, AdvisoryKind, HistoryEntry};
use crate::components::costs::cost_short;
use crate::model::{Entry, k};
use crate::ui::{CARNELIAN, FAIENCE, OCHRE, SAND};
use rpc_compose::api::clients::AgentSettingsApiClient;

/// Advisory metadata and disclosure belong to the owner, not core history.
pub struct Card {
    /// Canonical advisory receipt.
    pub advisory: Advisory,
    /// Show all notes or answer lines.
    pub expanded: bool,
}
/// Persistent remembered pick and selected-session advisor presentation.
#[derive(Default)]
pub struct Advisor {
    /// Last selected-session readback.
    pub status: AdvisorStatus,
    /// The remembered pick survives session resets.
    pub defaults: AdvisorDefaults,
    cards: HashMap<String, Card>,
    history_cards: usize,
    advisory_seqs: BTreeSet<i64>,
    awaiting_status: bool,
    refresh: bool,
}
/// Typed request retained on the real effect for existing scenario bindings.
#[derive(Clone, Debug)]
pub enum Request {
    /// Change the original selected session's advisor.
    SetAdvisor {
        /// Original advised session.
        session: SessionId,
        /// Advisor model, absent when off.
        model: Option<String>,
        /// Explicit effort, absent for the default.
        effort: Option<String>,
    },
    /// Read an existing link.
    AdvisorStatus(SessionId),
    /// Engine-wide consultation cap.
    SetAdvisorMaxConsults(u8),
}
/// Owner-local readbacks, live receipts and picker completion.
pub enum Msg {
    /// RPC readback; explicit `/advisor status` determines transcript output.
    Status(AdvisorStatus),
    /// Live link status, also printed in headless mode.
    Changed {
        /// Session whose link changed.
        session: SessionId,
        /// Authoritative link status.
        status: AdvisorStatus,
    },
    /// Live advisory receipt, deduplicated within its selected session.
    Added {
        /// Session receiving the advice.
        session: SessionId,
        /// Engine history sequence, also identifying an injected turn.
        seq: i64,
        /// Canonical advisory receipt.
        advisory: Advisory,
    },
    /// Original picker endpoint, not the session selected when it completes.
    Picked {
        /// Requesting session, absent before the first message.
        session: Option<SessionId>,
        /// Model or effort chip selected by the shared picker.
        pick: models::Pick,
    },
    /// Effective engine consultation cap.
    Consults(u8),
    /// Preserve generic command-error transitions.
    Failed(String),
}
impl Advisor {
    /// Preserve the fullscreen boot default; the old line printer starts unseeded.
    pub fn configured(info: Option<&rpc_compose::api::Info>, interactive: bool) -> Self {
        Self { defaults: info.filter(|_| interactive).map(|info| info.advisor.clone()).unwrap_or_default(), ..Self::default() }
    }
    fn request(request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            let result = match request {
                Request::SetAdvisor { session, model, effort } => api.set_advisor(session, model, effort).await.map(Msg::Status),
                Request::AdvisorStatus(session) => api.advisor_status(session).await.map(Msg::Status),
                Request::SetAdvisorMaxConsults(n) => api.set_advisor_max_consults(n).await.map(|defaults| Msg::Consults(defaults.max_consults)),
            };
            result.unwrap_or_else(|error| Msg::Failed(crate::reason(error)))
        });
    }
    fn command(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.harness.is_some() {
            cx.core(CoreAction::Reject("/advisor: not available for an external harness session".into()));
            return;
        }
        if arg.is_empty() {
            let session = cx.core.session;
            let (to, engine_epoch) = (cx.token, cx.epoch);
            let selection = models::Selection::external(
                "advisor".into(), self.status.model.clone(), self.status.effort.clone(), true, false, true,
                Box::new(move |pick| Some(CoreAction::Post(LocalReply {
                    to, engine_epoch, body: Box::new(Msg::Picked { session, pick }),
                }))),
            );
            cx.send_to::<models::Models>(models::Msg::Open { selection, chips: false });
            return;
        }
        if let Some(n) = arg.strip_prefix("consults") {
            match n.trim().parse::<u8>() {
                Ok(n) if n <= 32 => Self::request(Request::SetAdvisorMaxConsults(n), cx),
                _ => cx.core(CoreAction::Reject(format!("/advisor consults: expected a number 0-32, got {:?}", n.trim()))),
            }
            return;
        }
        let Some(session) = cx.core.session else {
            cx.core(CoreAction::Reject("no session yet: send a message first".into()));
            return;
        };
        let (model, effort) = match arg {
            "status" => {
                self.awaiting_status = true;
                Self::request(Request::AdvisorStatus(session), cx);
                return;
            }
            "off" => (None, None),
            "on" => match self.defaults.model.clone() {
                Some(model) => (Some(model), self.defaults.effort.clone()),
                None => {
                    cx.core(CoreAction::Reject("/advisor on: no remembered advisor model — pick one with /advisor".into()));
                    return;
                }
            },
            id => (Some(id.to_owned()), None),
        };
        self.set_advisor(session, model, effort, cx);
    }
    fn set_advisor(&mut self, session: SessionId, model: Option<String>, effort: Option<String>, cx: &mut Cx<'_, '_, Msg>) {
        self.awaiting_status = false;
        Self::request(Request::SetAdvisor { session, model, effort }, cx);
    }
    fn install(&mut self, status: AdvisorStatus, cx: &mut Cx<'_, '_, Msg>) {
        let off = |s: &AdvisorStatus| s.state == AdvisorState::Off || s.model.is_none();
        let changed = status.model != self.status.model || off(&status) != off(&self.status);
        let warn = status.warning.is_some() && !off(&status) && (changed || status.warning != self.status.warning);
        if status.model.is_some() {
            self.defaults.model.clone_from(&status.model);
            self.defaults.effort.clone_from(&status.effort);
        }
        self.status = status;
        cx.send_to::<models::Models>(models::Msg::Current {
            noun: "advisor".into(), model: self.status.model.clone(), effort: self.status.effort.clone(),
        });
        if changed { cx.interject(advisor_changed_line(&self.status)); }
        if let Some(warning) = self.status.warning.as_deref().filter(|_| warn) {
            cx.interject(format!("advisor: {warning}"));
        }
    }
    fn fence(cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
    }
    /// Join an owner's card state to its existing stable core entry identity.
    pub fn card(&self, entry: &Entry) -> Option<&Card> { self.cards.get(entry.key()?) }
    /// Consume a receipt once; generic core retains only its rendered fallback.
    fn advisory_card(&mut self, advisory: Advisory, key: uuid::Uuid) -> TranscriptCard {
        let entry_key = format!("advisor:{key}");
        let card = TranscriptCard {
            group: "advisor", key, entry_key: Some(entry_key.clone()), glyph: ADVISOR_GLYPH,
            color: severity_color(advisory_severity(&advisory)), title: advisory_title(&advisory),
            body: markdown::Text::complete(&advisory_lines(&advisory).join("\n")), expanded: false, plain: true,
        };
        self.cards.insert(entry_key, Card { advisory, expanded: false });
        card
    }
    /// Exact original state segment, on the shared animation clock.
    pub fn segment(&self, frame: usize) -> Option<(String, AdvisorState)> {
        let state = self.status.state;
        let word = match state {
            AdvisorState::Off => return None,
            AdvisorState::Reviewing => format!("{} reviewing…", spinner(frame)),
            state => advisor_state_name(state).to_owned(),
        };
        Some((format!("  ·  {ADVISOR_GLYPH} advisor: {word}"), state))
    }
}
impl Component for Advisor {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "advisor", slash: &[("/advisor", "[off|on|status|consults <n>|<provider/model>]", "a second model that reviews this session and answers its consults (picker without argument)")], bindings: &[],
            interests: &[Interest { topic: "agent.events", version: 1, variants: &["AdvisorChanged", "AdvisoryAdded"] }] }
    }
    fn decode(&self, _: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        match core.agent? {
            agent::AgentEvent::AdvisorChanged { session, status } => Some(Msg::Changed { session: *session, status: status.clone() }),
            agent::AgentEvent::AdvisoryAdded { session, seq, advisory } => Some(Msg::Added { session: *session, seq: *seq, advisory: advisory.clone() }),
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => self.command(args, cx),
            Input::Message(Msg::Picked { session, pick }) => match (session, pick) {
                (Some(session), models::Pick::Model(model, effort)) |
                (Some(session), models::Pick::Effort(model @ Some(_), effort)) => self.set_advisor(session, model, effort, cx),
                (None, _) => cx.core(CoreAction::Reject("no session yet: send a message first".into())),
                _ => {}
            },
            Input::Message(Msg::Status(status)) => {
                Self::fence(cx);
                for line in advisor_status_lines(&status) { cx.plain(Plain::Line(line)); }
                if cx.core.session.is_some_and(|session| session != status.session) { return; }
                let show = std::mem::take(&mut self.awaiting_status);
                self.install(status, cx);
                if show { for line in advisor_status_lines(&self.status) { cx.core(CoreAction::Notice(line)); } }
            }
            Input::Message(Msg::Changed { session, status }) => {
                cx.claim_plain();
                Self::fence(cx);
                cx.plain(Plain::Line(advisor_changed_line(&status)));
                if cx.core.session == Some(session) { self.install(status, cx); }
            }
            Input::Message(Msg::Added { session, seq, advisory }) => {
                cx.claim_plain();
                Self::fence(cx);
                cx.plain(Plain::Line(advisory_title(&advisory)));
                for line in advisory_lines(&advisory) { cx.plain(Plain::Line(line)); }
                if cx.core.session == Some(session) && self.advisory_seqs.insert(seq) {
                    cx.core(CoreAction::InterjectCard(self.advisory_card(advisory, uuid::Uuid::from_u128((1_u128 << 127) | u128::from(seq as u64)))));
                }
            }
            Input::Message(Msg::Consults(n)) => {
                Self::fence(cx);
                let line = format!("advisor consults → {n}");
                cx.plain(Plain::Line(line.clone()));
                cx.core(CoreAction::Notice(line));
            }
            Input::Message(Msg::Failed(error)) => cx.error(error),
            Input::Lifecycle(Lifecycle::SessionReset) => {
                self.status = AdvisorStatus::default();
                self.awaiting_status = false;
                self.refresh = false;
                self.cards.clear();
                self.history_cards = 0;
                self.advisory_seqs.clear();
            }
            Input::Lifecycle(Lifecycle::TranscriptReplaced) => { self.cards.clear(); self.history_cards = 0; },
            Input::Lifecycle(Lifecycle::SessionRead(_)) => self.refresh = true,
            Input::Lifecycle(Lifecycle::Resync) => self.refresh = cx.core.session.is_some(),
            Input::Tick { .. } if std::mem::take(&mut self.refresh) => {
                if cx.core.interactive && let Some(session) = cx.core.session { Self::request(Request::AdvisorStatus(session), cx); }
            }
            Input::EntryAction { key, operation: EntryOperation::Toggle } => {
                if let Some(card) = self.cards.get_mut(key) {
                    card.expanded = !card.expanded;
                    cx.consume_key();
                }
            }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Header(HeaderSlot::Advisor) => match (&self.status.state, &self.status.cost) {
                (AdvisorState::Off, _) | (_, None) => None,
                (_, Some(cost)) => Some(View::Text(format!("  ·  advisor {}", cost_short(cost)))),
            },
            Surface::Status(StatusSlot::AfterCore) => {
                use iocraft::prelude::*;
                self.segment(cx.frame).map(|(text, state)| super::View::Element {
                    element: element!(Text(content: text, color: advisor_color(state))).into_any(), rows: 1, replace: false,
                })
            },
            Surface::Entry { key, .. } => {
                let card = self.cards.get(key)?;
                Some(View::Element { element: render_card(&card.advisory, card.expanded), rows: card_height(card), replace: true })
            }
            _ => None,
        }
    }
    fn wake(&self) -> Option<u64> { self.refresh.then_some(0) }
    fn animating(&self) -> bool { self.status.state == AdvisorState::Reviewing }
    fn session_hidden(&self, session: SessionId) -> bool { self.status.advisor == Some(session) }
    fn turn_hidden(&self, seq: i64) -> bool { self.advisory_seqs.contains(&seq) }
    fn wrapper_tags(&self) -> &'static [&'static str] { &["advisory"] }
    fn wrapper(&self, _: Wrapper<'_>) -> WrapperView { WrapperView::Hidden }
    fn entry_height(&self, key: &str, _: usize) -> Option<usize> { self.cards.get(key).map(card_height) }
    fn entry_toggleable(&self, key: &str) -> Option<bool> {
        self.cards.get(key).map(|card| advisory_row_count(&card.advisory) > ADVISORY_COLLAPSED)
    }
    fn history_entry(&mut self, row: HistoryEntry, _: &markdown::Options) -> Result<Vec<Entry>, HistoryEntry> {
        match row {
            HistoryEntry::Advisory { advisory, .. } => {
                let key = uuid::Uuid::from_u128(self.history_cards as u128);
                self.history_cards += 1;
                Ok(vec![self.advisory_card(advisory, key).into()])
            },
            row => Err(row),
        }
    }
    fn history_plain(&self, row: &HistoryEntry) -> Option<Vec<String>> {
        let HistoryEntry::Advisory { advisory, .. } = row else { return None };
        let mut lines = vec![advisory_title(advisory)];
        lines.extend(advisory_lines(advisory));
        Some(lines)
    }
}
fn card_height(card: &Card) -> usize {
    let n = advisory_row_count(&card.advisory);
    if card.expanded { 4 + n } else { 4 + n.min(ADVISORY_COLLAPSED) + usize::from(n > ADVISORY_COLLAPSED) }
}

/// Notes an [`Card`] shows before Ctrl+O (125 §7.4).
pub const ADVISORY_COLLAPSED: usize = 3;
/// The advisor glyph (U+25CE; outside `tests/no_emoji.rs`' ranges, `◉` is Vision).
pub const ADVISOR_GLYPH: &str = "◎";

/// `advice` / `nit` / `concern` / `blocker`: the badge word.
pub fn severity_name(s: AdvisorSeverity) -> &'static str {
    match s {
        AdvisorSeverity::Advice => "advice",
        AdvisorSeverity::Nit => "nit",
        AdvisorSeverity::Concern => "concern",
        AdvisorSeverity::Blocker => "blocker",
    }
}

/// The card's severity: the worst of its notes (`Nit` for an empty list).
pub fn advisory_severity(a: &Advisory) -> AdvisorSeverity {
    a.notes
        .iter()
        .map(|n| n.severity)
        .max()
        .unwrap_or(AdvisorSeverity::Nit)
}

/// `◎ advisor · 2 notes`, or `◎ consulted: <question>` for a consultation
/// (R1.2; the question collapsed to one line).
pub fn advisory_title(a: &Advisory) -> String {
    match &a.kind {
        AdvisoryKind::Consultation { question } => {
            let q: Vec<&str> = question.split_whitespace().collect();
            format!("{ADVISOR_GLYPH} consulted: {}", q.join(" "))
        }
        AdvisoryKind::Review => {
            let n = a.notes.len();
            format!(
                "{ADVISOR_GLYPH} advisor · {n} note{}",
                if n == 1 { "" } else { "s" }
            )
        }
    }
}

/// The card's body rows: one per note with its badge for a review; one per
/// line of the answer, unbadged, for a consultation (R1.2). `ADVISORY_COLLAPSED`
/// of them show until Ctrl+O.
pub fn advisory_rows(a: &Advisory) -> Vec<(Option<AdvisorSeverity>, &str)> {
    match a.kind {
        AdvisoryKind::Consultation { .. } => a
            .notes
            .iter()
            .flat_map(|n| n.note.lines())
            .map(|l| (None, l))
            .collect(),
        AdvisoryKind::Review => a
            .notes
            .iter()
            .map(|n| (Some(n.severity), n.note.as_str()))
            .collect(),
    }
}

/// `advisory_rows(a).len()` without building them (`Entry::height`, Ctrl+O).
pub fn advisory_row_count(a: &Advisory) -> usize {
    match a.kind {
        AdvisoryKind::Consultation { .. } => a.notes.iter().map(|n| n.note.lines().count()).sum(),
        AdvisoryKind::Review => a.notes.len(),
    }
}

/// `[concern] note text` / an answer line: one line per row (the headless
/// printer; the renderer colours the badge itself).
pub fn advisory_lines(a: &Advisory) -> Vec<String> {
    advisory_rows(a)
        .into_iter()
        .map(|(sev, text)| match sev {
            Some(s) => format!("[{}] {text}", severity_name(s)),
            None => text.to_owned(),
        })
        .collect()
}

/// `advisor → anthropic/claude-opus-4-1` / `advisor → off`: the one info
/// line on `AdvisorChanged` (125 §7.5, the `model → …` shape).
pub fn advisor_changed_line(s: &AdvisorStatus) -> String {
    match (&s.state, &s.model) {
        (AdvisorState::Off, _) | (_, None) => "advisor → off".to_owned(),
        (_, Some(m)) => format!("advisor → {m}"),
    }
}

/// `off` / `idle` / `reviewing` / `paused` / `halted`.
pub fn advisor_state_name(s: AdvisorState) -> &'static str {
    match s {
        AdvisorState::Off => "off",
        AdvisorState::Idle => "idle",
        AdvisorState::Reviewing => "reviewing",
        AdvisorState::Paused => "paused",
        AdvisorState::Halted => "halted",
    }
}

/// `/advisor status` (125 §7.3 + R1.2): the five lines, `—` for unknowns;
/// one line when off.
pub fn advisor_status_lines(s: &AdvisorStatus) -> Vec<String> {
    if s.state == AdvisorState::Off {
        return vec!["advisor: off  ·  /advisor to pick a model".to_owned()];
    }
    let dash = || "—".to_owned();
    // Why paused / halted rides on the state line.
    let detail = s
        .detail
        .as_deref()
        .map_or_else(String::new, |d| format!("  ·  {d}"));
    let mut out = vec![
        format!(
            "advisor: {}  ·  {}  ·  {}{detail}",
            advisor_state_name(s.state),
            s.model.clone().unwrap_or_else(dash),
            s.effort.clone().unwrap_or_else(dash)
        ),
        format!(
            "notes: {} nit · {} concern · {} blocker  ·  step {}  ·  backlog {}",
            s.notes.nit, s.notes.concern, s.notes.blocker, s.step, s.backlog
        ),
    ];
    let context = if s.context_limit > 0 {
        format!(
            "{} / {} ({}%)",
            k(s.context_used),
            k(s.context_limit),
            u64::from(s.context_used) * 100 / u64::from(s.context_limit)
        )
    } else {
        k(s.context_used)
    };
    out.push(format!(
        "context: {context}  ·  in {} · out {}  ·  cost {}",
        k(u32::try_from(s.tokens_in).unwrap_or(u32::MAX)),
        k(u32::try_from(s.tokens_out).unwrap_or(u32::MAX)),
        s.cost.as_ref().map_or_else(dash, cost_short)
    ));
    out.push(match &s.last_note {
        Some(n) => format!("last: [{}] {}", severity_name(n.severity), n.note),
        None => "last: —".to_owned(),
    });
    out.push(format!(
        "consults: {} this turn · {} total · cap {}",
        s.consults.turn, s.consults.total, s.consults.cap
    ));
    out
}


pub(crate) fn render_card(advisory: &Advisory, expanded: bool) -> AnyElement<'static> {
    use iocraft::prelude::*;
    let color = severity_color(advisory_severity(advisory));
    let all = advisory_rows(advisory);
    let n = all.len();
    let shown = if expanded { n } else { n.min(ADVISORY_COLLAPSED) };
    let title = advisory_title(advisory);
    let header = if n <= ADVISORY_COLLAPSED {
        title
    } else if expanded {
        format!("{title}   ctrl+o collapses")
    } else {
        format!("{title}   ctrl+o expands")
    };
    let rows: Vec<AnyElement<'static>> = all
        .iter()
        .take(shown)
        .map(|(sev, text)| match sev {
            Some(sev) => element!(MixedText(contents: vec![
                MixedTextContent::new(format!("[{}]", severity_name(*sev))).color(severity_color(*sev)),
                MixedTextContent::new(format!(" {text}")),
            ], wrap: TextWrap::Wrap))
            .into_any(),
            None => element!(Text(content: (*text).to_owned(), wrap: TextWrap::Wrap)).into_any(),
        })
        .collect();
    let more = (n > shown).then(|| {
        let noun = match advisory.kind {
            AdvisoryKind::Consultation { .. } => "lines",
            AdvisoryKind::Review => "notes",
        };
        format!("… +{} more {noun}", n - shown)
    });
    element! {
        View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: color, padding_left: 1, padding_right: 1, margin_top: 1) {
            Text(content: header, color, weight: Weight::Bold)
            #(rows)
            #(more.map(|m| element!(Text(content: m, color: SAND))))
        }
    }
    .into_any()
}

/// The advisor palette (125 §7.4): advice and nit SAND (muted), concern
/// OCHRE (warning), blocker CARNELIAN (danger).
pub(crate) fn severity_color(s: AdvisorSeverity) -> iocraft::Color {
    match s {
        AdvisorSeverity::Advice | AdvisorSeverity::Nit => SAND,
        AdvisorSeverity::Concern => OCHRE,
        AdvisorSeverity::Blocker => CARNELIAN,
    }
}

/// The status segment's colour by advisor state (125 §7.5): idle SAND,
/// reviewing FAIENCE, paused OCHRE, halted CARNELIAN.
fn advisor_color(s: AdvisorState) -> iocraft::Color {
    match s {
        AdvisorState::Off | AdvisorState::Idle => SAND,
        AdvisorState::Reviewing => FAIENCE,
        AdvisorState::Paused => OCHRE,
        AdvisorState::Halted => CARNELIAN,
    }
}

