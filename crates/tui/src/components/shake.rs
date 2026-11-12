//! Active context category picker/review; core retains mutation and reload transactions.
use std::collections::BTreeSet;
use agent::{ContextItem, ContextKind, ContextSnapshot, ContextTarget};
use rpc_compose::api::clients::TurnsApiClient;
use super::*;

#[derive(Default)]
pub(crate) struct ShakePanel { pub menu: Option<Box<Shake>> }
#[derive(Clone)]
pub(crate) enum Request { ContextItems { session: SessionId, category: Option<ShakeCategory> } }
pub(crate) enum Msg { Loaded { session: SessionId, category: Option<ShakeCategory>, result: Result<ContextSnapshot, String> } }


/// The `/shake` argument: which kind of context item the picker lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShakeCategory {
    /// Image occurrences: user images, tool-response screenshots, snap-head PNGs.
    Images,
    /// Native PDF parts.
    Documents,
    /// Whole messages; nothing preselected.
    Messages,
    /// Tool exchanges with their protocol groups.
    Tools,
    /// The current epoch head.
    Summary,
}

impl ShakeCategory {
    /// The literal `/shake` argument; `None` for anything else (the caller prints usage).
    pub fn parse(arg: &str) -> Option<Self> {
        Some(match arg {
            "images" => Self::Images,
            "documents" => Self::Documents,
            "messages" => Self::Messages,
            "tools" => Self::Tools,
            "summary" => Self::Summary,
            _ => return None,
        })
    }

    /// The inventory kind this category lists.
    fn kind(self) -> ContextKind {
        match self {
            Self::Images => ContextKind::Image,
            Self::Documents => ContextKind::Document,
            Self::Messages => ContextKind::Message,
            Self::Tools => ContextKind::Tool,
            Self::Summary => ContextKind::Summary,
        }
    }

    /// Plural noun for the zero-matches notice.
    fn noun(self) -> &'static str {
        match self {
            Self::Images => "images",
            Self::Documents => "documents",
            Self::Messages => "messages",
            Self::Tools => "tool exchanges",
            Self::Summary => "summary",
        }
    }
}

/// Which `/shake` screen is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShakeStage {
    /// Ticking inventory rows.
    Pick,
    /// The exact removal set, awaiting ⏎.
    Review,
}

/// `/shake` state (48 §2.4): the server's whole active inventory, what the
/// picker lists and what is ticked. Targets come from the inventory only;
/// the transcript on screen is never a deletion handle.
#[derive(Debug)]
pub struct Shake {
    /// Session the inventory came from.
    pub session: SessionId,
    /// Inventory revision; the removal is refused if the session moved on.
    pub revision: String,
    /// The inventory, in server order.
    pub items: Vec<ContextItem>,
    /// Indices into `items` the picker lists: the category's kind, or all.
    pub shown: Vec<usize>,
    /// Ticked indices into `items`.
    pub selected: BTreeSet<usize>,
    /// Highlighted row of `shown` (pick) or `review` (review).
    pub cursor: usize,
    /// Which screen is up.
    pub stage: ShakeStage,
    /// The removal set, built when review opens.
    pub review: Vec<ReviewRow>,
}

/// One line of the `/shake` review: an exact target and why it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRow {
    /// What `remove_context` gets.
    pub target: ContextTarget,
    /// Inventory kind (`Message` for a companion the inventory does not list).
    pub kind: ContextKind,
    /// The inventory label.
    pub label: String,
    /// Not ticked: a protocol companion of a ticked message (48 §4.2).
    pub companion: bool,
}

/// Lowercase kind for picker rows and the print-mode inventory.
pub fn kind_name(k: ContextKind) -> &'static str {
    match k {
        ContextKind::Message => "message",
        ContextKind::Image => "image",
        ContextKind::Document => "document",
        ContextKind::Tool => "tool",
        ContextKind::Summary => "summary",
    }
}

/// The message or head a binary target sits in.
fn owner_target(t: ContextTarget) -> Option<ContextTarget> {
    match t {
        ContextTarget::Binary { owner, .. } => Some(match owner {
            agent::ContextOwner::Message { seq } => ContextTarget::Message { seq },
            agent::ContextOwner::Head { epoch } => ContextTarget::Head { epoch },
        }),
        _ => None,
    }
}

/// The exact removal set (48 §3.2): the ticked items plus their
/// `remove_with` companions, followed to closure, each target once, in
/// inventory order. A whole message or head subsumes its attachments, so a
/// ticked image inside a ticked message is not listed twice.
pub fn expand(items: &[ContextItem], selected: &BTreeSet<usize>) -> Vec<ReviewRow> {
    let picked: BTreeSet<ContextTarget> = selected
        .iter()
        .filter_map(|&i| items.get(i))
        .map(|it| it.target)
        .collect();
    let mut companions = BTreeSet::new();
    let mut queue: Vec<ContextTarget> = picked.iter().copied().collect();
    while let Some(t) = queue.pop() {
        for &c in items
            .iter()
            .filter(|it| it.target == t)
            .flat_map(|it| &it.remove_with)
        {
            if !picked.contains(&c) && companions.insert(c) {
                queue.push(c);
            }
        }
    }
    let subsumed = |t: ContextTarget| {
        owner_target(t).is_some_and(|o| picked.contains(&o) || companions.contains(&o))
    };
    let mut rows: Vec<ReviewRow> = items
        .iter()
        .filter(|it| {
            (picked.contains(&it.target) || companions.contains(&it.target)) && !subsumed(it.target)
        })
        .map(|it| ReviewRow {
            target: it.target,
            kind: it.kind,
            label: it.label.clone(),
            companion: !picked.contains(&it.target),
        })
        .collect();
    // A companion the inventory does not list (the server names a row it
    // hid): still part of the set, labelled by its coordinate.
    for &c in &companions {
        if !subsumed(c) && !rows.iter().any(|r| r.target == c) {
            rows.push(ReviewRow {
                target: c,
                kind: ContextKind::Message,
                label: match c {
                    ContextTarget::Message { seq } => format!("message #{seq}"),
                    ContextTarget::Head { epoch } => format!("summary (epoch {epoch})"),
                    ContextTarget::Binary { part, .. } => format!("attachment part {part}"),
                },
                companion: true,
            });
        }
    }
    rows
}

impl Shake {
    /// Tick or untick the highlighted picker row.
    fn toggle(&mut self) {
        if let Some(&i) = self.shown.get(self.cursor)
            && !self.selected.remove(&i)
        {
            self.selected.insert(i);
        }
    }

    /// Build the removal set and show it.
    fn open_review(&mut self) {
        self.review = expand(&self.items, &self.selected);
        self.stage = ShakeStage::Review;
        self.cursor = 0;
    }

    /// `(messages, attachments, heads, companions)` of the review.
    fn counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0);
        for r in &self.review {
            match r.target {
                ContextTarget::Message { .. } => c.0 += 1,
                ContextTarget::Binary { .. } => c.1 += 1,
                ContextTarget::Head { .. } => c.2 += 1,
            }
            c.3 += usize::from(r.companion);
        }
        c
    }

    /// Picker rows: `[x] image     shot.png · +2 messages` (the companions a
    /// tick pulls in); review rows: the set with companions and summary
    /// warnings spelled out.
    pub fn rows(&self) -> Vec<String> {
        match self.stage {
            ShakeStage::Pick => self
                .shown
                .iter()
                .map(|&i| {
                    let it = &self.items[i];
                    let more = match it.remove_with.len() {
                        0 => String::new(),
                        n => format!(" · +{n} message(s)"),
                    };
                    format!(
                        "[{}] {:<8} {}{more}",
                        if self.selected.contains(&i) { "x" } else { " " },
                        kind_name(it.kind),
                        it.label
                    )
                })
                .collect(),
            ShakeStage::Review => self
                .review
                .iter()
                .map(|r| {
                    let note = match r.target {
                        ContextTarget::Head { .. } => " · summarized earlier context will be lost",
                        ContextTarget::Binary {
                            owner: agent::ContextOwner::Head { .. },
                            ..
                        } => {
                            " · summary image: removing this loses part of the earlier conversation"
                        }
                        _ if r.companion => " · protocol companion",
                        _ => "",
                    };
                    format!("{:<8} {}{note}", kind_name(r.kind), r.label)
                })
                .collect(),
        }
    }

    /// Menu title: keys and, in review, the counts and what removal means.
    pub fn title(&self) -> String {
        match self.stage {
            ShakeStage::Pick => format!(
                "shake  ↑↓ move  space tick  ⏎ review  esc close   {} ticked",
                self.selected.len()
            ),
            ShakeStage::Review => {
                let (m, a, h, c) = self.counts();
                format!(
                    "remove from context: {m} message(s), {a} attachment(s), {h} head(s), {c} companion(s)  ⏎ remove  esc back   originals stay in history; later replies stay; /undo does not restore"
                )
            }
        }
    }
}
impl ShakePanel {

    /// `/shake [images|documents|messages|tools|summary]` (48 §2.1): ask the
    /// server for the whole active inventory; the reply opens the picker.
    /// An unknown word is usage, never chat input or a skill.
    fn shake_cmd(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        let category = match arg {
            "" => None,
            a => match ShakeCategory::parse(a) {
                Some(c) => Some(c),
                None => {
                    cx.core(CoreAction::Reject(SHAKE_USAGE.into()));
                    return;
                }
            },
        };
        match cx.core.session {
            Some(session) => cx.spawn_request(Lane::Concurrent, Request::ContextItems { session, category },
                |api, Request::ContextItems { session, category }| async move {
                    Msg::Loaded { session, category, result: api.context_items(session).await.map_err(crate::reason) }
                }),
            None => cx.core(CoreAction::Notice("no session".into())),
        }
    }

    /// Open the `/shake` picker over `snapshot`: every item for the bare
    /// command and `messages` with nothing ticked; a category's items ticked
    /// and straight into review otherwise. A refusal (busy, external
    /// harness) and an empty match are one notice each; nothing is written.
    fn open_shake(
        &mut self,
        session: SessionId,
        category: Option<ShakeCategory>,
        snapshot: ContextSnapshot,
        cx: &mut Cx<'_, '_, Msg>,
    ) {
        if cx.core.session != Some(session) || cx.core.inspecting {
            return;
        }
        cx.core(CoreAction::ContextState { session, revision: snapshot.revision.clone(), editable: snapshot.editable,
            reason: if snapshot.editable { None } else { snapshot.reason.clone() } });
        if !snapshot.editable {
            cx.core(CoreAction::Notice(format!(
                "shake: {}",
                snapshot
                    .reason
                    .as_deref()
                    .unwrap_or("context is not editable")
            )));
            return;
        }
        let shown: Vec<usize> = (0..snapshot.items.len())
            .filter(|&i| category.is_none_or(|c| snapshot.items[i].kind == c.kind()))
            .collect();
        if shown.is_empty() {
            cx.core(CoreAction::Notice(match category {
                Some(c) => format!("no {} in current context", c.noun()),
                None => "nothing in current context".into(),
            }));
            return;
        }
        let selected: BTreeSet<usize> = match category {
            None | Some(ShakeCategory::Messages) => BTreeSet::new(),
            Some(_) => shown.iter().copied().collect(),
        };
        let mut shake = Shake {
            session,
            revision: snapshot.revision,
            items: snapshot.items,
            shown,
            selected,
            cursor: 0,
            stage: ShakeStage::Pick,
            review: Vec::new(),
        };
        if !shake.selected.is_empty() {
            shake.open_review();
        }
        self.menu = Some(Box::new(shake));
        cx.focus(FocusClass::Modal);
    }

    /// `/shake` keys: ↑↓ move, space ticks a picker row, ⏎ reviews then
    /// removes, esc backs out of review then closes. Nothing is sent
    /// before the review's ⏎.
    fn shake_key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some(s) = &mut self.menu else {
            return;
        };
        let len = match s.stage {
            ShakeStage::Pick => s.shown.len(),
            ShakeStage::Review => s.review.len(),
        };
        match (code, s.stage) {
            (KeyCode::Up, _) => s.cursor = s.cursor.saturating_sub(1),
            (KeyCode::Down, _) => s.cursor = (s.cursor + 1).min(len.saturating_sub(1)),
            (KeyCode::Char(' '), ShakeStage::Pick) => s.toggle(),
            (KeyCode::Enter, ShakeStage::Pick) => {
                if !s.selected.is_empty() {
                    s.open_review();
                }
            }
            (KeyCode::Esc, ShakeStage::Review) => {
                s.stage = ShakeStage::Pick;
                s.cursor = 0;
            }
            (KeyCode::Esc, ShakeStage::Pick) => { self.menu = None; cx.close(); },
            (KeyCode::Enter, ShakeStage::Review) => {
                let cmd = CoreAction::EditContext {
                    session: s.session,
                    expected_revision: s.revision.clone(),
                    targets: s.review.iter().map(|r| r.target).collect(),
                };
                self.menu = None;
                cx.close();
                cx.core(cmd);
            }
            _ => {}
        }
    }
}

/// The `/shake` usage line, shown on an unknown argument (48 §2.1).
pub const SHAKE_USAGE: &str = "usage: /shake [images|documents|messages|tools|summary]";
impl Component for ShakePanel {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "shake", interests: &[], bindings: &[],
            slash: &[("/shake", "[images|documents|messages|tools|summary]", "remove images, messages or the summary from the model's context")] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } if !cx.core.inspecting => self.shake_cmd(args, cx),
            Input::Message(Msg::Loaded { session, category, result }) => match result {
                Err(error) => cx.error(error),
                Ok(snapshot) => {
                    use std::fmt::Write;
                    let mut text = if cx.core.reasoning { "\x1b[0m\nmenus need a TTY\n".into() } else { "menus need a TTY\n".to_owned() };
                    for item in &snapshot.items {
                        let _ = writeln!(text, "{:<8} {}{}", kind_name(item.kind), item.label,
                            match item.remove_with.len() { 0 => String::new(), n => format!(" · +{n} message(s)") });
                    }
                    cx.plain(Plain::Text(text));
                    self.open_shake(session, category, snapshot, cx);
                }
            },
            Input::Key(key) if self.menu.is_some() && !cx.core.inspecting => { self.shake_key(key.code, cx); cx.consume_key(); }
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::FocusLost) => { self.menu = None; cx.close(); }
            Input::Lifecycle(Lifecycle::History(_)) if self.menu.take().is_some() => {
                cx.close();
                cx.core(CoreAction::Notice("context changed — /shake again to review".into()));
            }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        if !matches!(at, Surface::Modal) || cx.core.inspecting { return None; }
        let menu = self.menu.as_ref()?;
        let rows = menu.rows();
        Some(View::Element { rows: rows.len().min(10) + 3, replace: false,
            element: crate::ui::menu_view(menu.title(), rows, menu.cursor, cx.width as u16, cx.height as u16, false) })
    }
}
