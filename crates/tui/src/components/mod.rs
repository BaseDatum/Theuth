//! Typed synchronous components; only registry storage and local replies erase Msg.
use std::{any::Any, future::Future, sync::Arc};
use futures::future::BoxFuture;
use iocraft::{KeyCode, KeyEvent, KeyModifiers, AnyElement};
use rpc::client::Engine;
use runtime::SessionId;
use crate::{model::UiEvent, topics::TopicEvent};

mod registry;
pub mod update;

// Step-11 group t1 (network and prompts): add modules below, in this block only.
pub mod computer;
pub mod sandboxes;
pub mod link;
pub mod nodes;
pub mod mesh_devices;
pub mod secrets;
pub mod peers;

// Step-11 group t2 (boards, costs and artifacts): add modules below, in this block only.
pub mod tasks;
pub mod roster;
pub mod routines;
pub mod autoresearch;
pub mod costs;
pub mod web_progress;
pub mod artifacts_apps;
pub mod double;
pub mod todos;
pub mod shake;

// Step-11 group t3 (jobs, terminals, speech and search): add modules below, in this block only.
pub mod jobs;
pub mod terminals;
pub mod broadcast;
pub mod knowledge;
pub mod search_attach;

// Model-picker panels (after design 194): add modules below, in this block only.
pub use registry::{Registry, builtin};

#[cfg(test)]
pub(crate) mod tests;

/// A stable slot identity; tombstones reject replies after removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentToken {
    /// Index never shifts while a reply can be in flight.
    pub slot: usize,
    /// Incremented before a removed slot could be reused.
    pub generation: u64,
}

/// Opaque, owner-typed completion. Debug never formats its potentially secret body.
pub struct LocalReply {
    /// Component that issued the effect.
    pub to: ComponentToken,
    /// Engine generation at issue time, not the current target.
    pub engine_epoch: u64,
    /// Only Mounted<C> downcasts this to C::Msg.
    pub body: Box<dyn Any + Send>,
}
impl std::fmt::Debug for LocalReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalReply").field("to", &self.to).field("engine_epoch", &self.engine_epoch).finish_non_exhaustive()
    }
}

/// Ordinary work is concurrent; terminal input shares one FIFO worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// No serialization between independent requests.
    Concurrent,
    /// Preserves key ordering alongside the existing PTY input commands.
    OrderedInput,
}
/// One generic task envelope, independent of RPC method/parameter/reply shapes.
pub struct Effect {
    /// Executor scheduling class.
    pub lane: Lane,
    /// Takes the current engine when execution begins.
    pub work: Box<dyn FnOnce(Arc<Engine>) -> BoxFuture<'static, LocalReply> + Send>,
    #[cfg(test)]
    request: Option<Box<dyn Any + Send>>,
}
impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Effect { .. }") }
}
#[cfg(test)]
impl Effect {
    pub(crate) fn request<R: 'static>(&self) -> Option<&R> {
        self.request.as_ref()?.downcast_ref()
    }
}

/// Deferred owner work; its original reply endpoint survives modal closure.
pub struct SessionContinuation {
    pub(crate) work: Box<dyn FnOnce(SessionId) -> Effect + Send>,
    pub(crate) fail: Box<dyn FnOnce(String) -> LocalReply + Send>,
}
impl std::fmt::Debug for SessionContinuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionContinuation { .. }")
    }
}

/// Ordered at the existing imperative key-handler boundaries, not by visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FocusClass {
    /// Terminal capture, before global Ctrl-C.
    Interactive,
    /// Talk/dictation capture, after global quit.
    Spoken,
    /// Masked request input, after tool approval.
    Secret,
    /// Dictionary suggestion, before ask_user.
    Suggestion,
    /// Pinned card, after core prompts.
    Pinned,
    /// Search editor, after clipboard/core edit shortcuts.
    Search,
    /// Modal menu, after path completion.
    Modal,
    /// Content view, before transcript scrolling/composer.
    Content,
}
/// One chord in a fixed input class; equal-class duplicates are rejected.
#[derive(Clone, Copy)]
pub struct Binding {
    /// Existing imperative-handler boundary.
    pub class: FocusClass,
    /// Physical/semantic key code.
    pub code: KeyCode,
    /// Exact modifier set.
    pub modifiers: KeyModifiers,
}
/// Slash name (including /), argument hint, help. Aliases are separate rows.
pub type SlashSpec = (&'static str, &'static str, &'static str);
/// Topic/version interest with optional inner-tag prefilter.
pub struct Interest {
    /// Exact owner-qualified topic.
    pub topic: &'static str,
    /// Supported inner contract version.
    pub version: u32,
    /// Empty accepts all variants; otherwise tags are checked before decoding.
    pub variants: &'static [&'static str],
}
/// Immutable routing declaration, validated before the UI starts.
pub struct Declaration {
    /// Unique fixed built-in id.
    pub id: &'static str,
    /// Multiple recipients may share a topic.
    pub interests: &'static [Interest],
    /// Include aliases as separate rows; help, completion and speech share these.
    pub slash: &'static [SlashSpec],
    /// Fixed-class keyboard bindings.
    pub bindings: &'static [Binding],
}

/// A focused component may borrow the still-mounted composer instead of capturing it.
pub struct Editor {
    /// Prompt line above the composer.
    pub hint: String,
    /// Hide the bytes typed into the shared editor.
    pub masked: bool,
    /// An empty line is a meaningful answer.
    pub submit_empty: bool,
}

/// Borrowed from disjoint core fields; never a mutable Model or stored event cache.
#[derive(Clone, Copy)]
pub struct CoreView<'a> {
    /// Current selected session; None for an unsent draft.
    pub session: Option<SessionId>,
    /// Selected session parent and active private-tree root, independent of board state.
    pub parent: Option<SessionId>,
    /// Root of the active private lease, when present.
    pub private_root: Option<SessionId>,
    /// Effective project of the selected root.
    pub project: Option<&'a str>,
    /// Historical inspection must not be overwritten by live recovery.
    pub inspecting: bool,
    /// Core turn-in-flight state before this event.
    pub streaming: bool,
    /// An open dim reasoning block in the line printer's pre-event state.
    pub reasoning: bool,
    /// Once-decoded agent event shared with the core reducer for this dispatch.
    pub agent: Option<&'a agent::AgentEvent>,
    /// Accepted transcript entries; components retain only their own projection.
    pub entries: &'a [crate::model::Entry],

    // Step-11 group t1 CoreView fields: add below, in this block only.

    // Step-11 group t2 CoreView fields: add below, in this block only.
    /// Whether this registry is mounted on an interactive terminal surface.
    pub interactive: bool,
    /// Selected session's presentation label, not a feature-owned row.
    pub session_name: Option<&'a str>,
    /// Current terminal width used when new markdown text starts.
    pub width: usize,
    /// Observed members of the active private lease, independent of visible rows.
    pub private_members: Option<&'a std::collections::HashSet<SessionId>>,
    /// Borrowed neutral identity metadata; no board DTO crosses this contract.
    pub members: &'a dyn SessionMembers,
    /// Identity of the shown core approval, otherwise the first ask.
    pub prompt: Option<uuid::Uuid>,
    /// Membership in accepted core prompt queues, independent of shown identity.
    pub prompts: &'a dyn PromptIds,


    /// Full terminal height; distinct from the transcript row budget.
    pub terminal_height: usize,

    /// Existing modal menu or intrinsic secret editor suspends lower-priority capture.
    pub modal_open: bool,

    // Step-11 group t3 CoreView fields: add below, in this block only.
    /// Measured transcript row budget at the current key dispatch boundary.
    pub viewport: usize,
}
/// Read-only identity metadata shared by core session membership.
pub struct SessionMember<'a> {
    /// Agent/session label.
    pub name: &'a str,
    /// Effective project when known.
    pub project: Option<&'a str>,
}
/// Borrowed lookup avoids allocating a second membership projection per dispatch.
pub trait SessionMembers: Sync {
    /// Find one known identity, independent of board visibility.
    fn member(&self, id: SessionId) -> Option<SessionMember<'_>>;
}
/// Borrowed identity lookup for accepted core prompts, never their feature data.
pub trait PromptIds: Sync {
    /// Whether an identity is pending in either accepted queue.
    fn contains(&self, id: &str) -> bool;
}
impl CoreView<'_> {
    /// Whether this view owns the session's private export capability.
    pub fn private_member(&self, id: SessionId) -> bool {
        self.private_root == Some(id) || self.private_members.is_some_and(|members| members.contains(&id))
    }
    /// Core markdown width and fence hook, computed only when requested by an owner.
    pub fn markdown_options(&self) -> markdown::Options { crate::model::markdown_options(self.width) }

    /// Only the newest card in a named replacement group may keep updating.
    pub fn card_current(&self, group: &str, key: uuid::Uuid) -> bool {
        self.entries.iter().rev().find_map(|entry| match entry {
            crate::model::Entry::Card {
                kind: crate::model::CardKind::Component { group: old, key, .. }, ..
            } if *old == group => Some(*key),
            _ => None,
        }) == Some(key)
    }
    /// Match the selected root or one of its accepted descendants.
    pub fn owns_session(&self, session: SessionId) -> bool {
        self.session == Some(session) || self.members.member(session).is_some()
    }
}

/// Notifications borrow accepted core data; retain only the owner's projection.
#[derive(Clone, Copy)]
pub enum Lifecycle<'a> {
    /// Component is mounted, before normal input.
    Mount,
    /// The registry already advanced its reply epoch.
    EngineChanged,
    /// Clear session-scoped state, not persistent global caches.
    SessionReset,
    /// Core adopted a different selected session; read the updated session context.
    SessionChanged,
    /// Release local resources and subscriptions.
    Shutdown,
    /// A lossy stream requires an authoritative re-read, never mutation replay.
    Resync,
    /// Accepted history, before the borrowed snapshot is dropped.
    History(&'a agent::Loaded),
    /// Old entry identities are no longer valid.
    TranscriptReplaced,
    /// Core editor bytes changed.
    ComposerEdited(&'a str),
    /// Core composer accepted a submission.
    ComposerSubmitted(&'a str),
    /// Selected pending categories replace state, including empty arrays.
    Prompts(&'a serde_json::Value),
    /// Preserve the old cross-panel generic-error transitions.
    CommandFailed(&'a str),

    // Step-11 group t1 Lifecycle variants: add below, in this block only.
    /// Another component or a residual core modal acquired exclusive input.
    FocusLost,

    // Step-11 group t2 Lifecycle variants: add below, in this block only.
    /// Core prompt queues changed; a resolver may name one completed identity.
    PromptChanged {
        /// Resolved core prompt id, absent for a request or local dismissal.
        resolved: Option<uuid::Uuid>,
    },

    /// Selected-session read, including a snapshot whose transcript reload is refused.
    SessionRead(&'a agent::Loaded),

    // Step-11 group t3 Lifecycle variants: add below, in this block only.
}
/// Existing per-entry operations, dispatched at their original core input boundaries.
#[derive(Clone, Copy)]
pub enum EntryOperation {
    /// Open a local representation of the selected transcript entry.
    Open,
    /// Expand or collapse the selected completed entry.
    Toggle,
}
/// Owner-local input; no feature message is added to the central UiEvent enum.
pub enum Input<'a, M> {
    /// A decoded wire message or typed local reply.
    Message(M),
    /// A key at the declared input-precedence boundary.
    Key(&'a KeyEvent),
    /// Bracketed paste addressed to the current focus owner.
    Paste {
        /// Original pasted bytes.
        text: &'a str,
        /// Byte cursor in the shell's separate editor, if one is visible.
        caret: usize,
    },
    /// A submitted line addressed to the current focus owner.
    Submit(&'a str),
    /// Declared builtin/alias invocation, before skill fallback.
    Slash {
        /// Name including the slash.
        name: &'a str,
        /// Remaining trimmed arguments.
        args: &'a str,
    },
    /// Core-selected transcript entry operation.
    EntryAction {
        /// Stable call identity, never a retained Vec index.
        key: &'a str,
        /// Open or toggle at the existing input boundary.
        operation: EntryOperation,
    },
    /// A due deadline from the existing UI loop.
    Tick {
        /// Current Unix milliseconds.
        now_ms: u64,
        /// Current animation frame.
        frame: usize,
    },
    /// Core lifecycle/admission notification.
    Lifecycle(Lifecycle<'a>),
}
impl<'a, M> Input<'a, M> {
    fn without_message<N>(self) -> Option<Input<'a, N>> {
        Some(match self {
            Self::Message(_) => return None,
            Self::Key(key) => Input::Key(key), Self::Paste { text, caret } => Input::Paste { text, caret },
            Self::Submit(text) => Input::Submit(text),
            Self::Slash { name, args } => Input::Slash { name, args },
            Self::EntryAction { key, operation } => Input::EntryAction { key, operation },
            Self::Tick { now_ms, frame } => Input::Tick { now_ms, frame },
            Self::Lifecycle(event) => Input::Lifecycle(event),
        })
    }
}


/// Stable header geometry, independent of registry composition order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderSlot {
    /// Session spend.
    Costs,
    /// Advisor spend.
    Advisor,
    /// Approval policy.
    Approvals,
    /// Peer communication.
    Communication,
    /// Paired phones.
    Link,
    /// Connected nodes.
    Nodes,
    /// Available update.
    Update,
}
impl HeaderSlot {
    /// The existing left-to-right header order.
    pub const ALL: [Self; 7] = [Self::Costs, Self::Advisor, Self::Approvals,
        Self::Communication, Self::Link, Self::Nodes, Self::Update];
}
/// Status contributions occupy fixed positions among retained core segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusSlot {
    /// Background work, immediately after the queued-message count.
    Work,
    /// Loaded capabilities, before edit/history hints.
    Catalog,
    /// Focused pinned-view hints, after history.
    Pinned,
    /// Spoken-view hint, before the attached-engine label.
    Spoken,
    /// Right-aligned status text, separate from the left status contributions.
    Right,
}

/// Fixed layout locations, never feature names.
#[derive(Clone, Copy)]
pub enum Surface<'a> {
    /// One explicitly ordered header segment.
    Header(HeaderSlot),
    /// Owner presentation beside one session row, independent of feature DTOs.
    SessionBadge(SessionId),
    /// A fixed position in the status line, independent of registration order.
    Status(StatusSlot),
    /// Transcript content replacement or append.
    Content,
    /// Rows pinned below the transcript.
    Pinned,
    /// Modal slot after core prompts.
    Modal,
    /// Composer-adjacent region; editor handles stay in App.
    Composer,
    /// Composer hint.
    Hint,
    /// Prompt-adjacent badge.
    PromptBadge,
    /// Stable transcript decoration location.
    Entry {
        /// Stable entry/call identity, never a Vec index.
        key: &'a str,
        /// Rows already clipped above this entry.
        skipped: usize,
    },

    // Step-11 group t1 Surface variants: add below, in this block only.
    /// Intrinsic prompt at an explicit input-precedence boundary, without displacing focus.
    Prompt(FocusClass),

    // Step-11 group t2 Surface variants: add below, in this block only.
    /// Owner text for an existing pending/running transcript summary row; no spinner or extra rows.
    EntrySummary {
        /// Stable pending/tool call identity.
        key: &'a str,
    },

    // Step-11 group t3 Surface variants: add below, in this block only.
    /// Owner activity while core streaming/compaction/vision has no priority.
    Activity,
}
/// Read-only layout context; no iocraft Hooks or editor handles.
#[derive(Clone, Copy)]
pub struct ViewCx<'a> {
    /// Pre-render core state.
    pub core: CoreView<'a>,
    /// Allocated columns.
    pub width: usize,
    /// Allocated rows.
    pub height: usize,
    /// Animation frame.
    pub frame: usize,
    /// Mounted shell mailbox; absent for detached read-only rendering.
    pub(crate) commands: Option<&'a tokio::sync::mpsc::UnboundedSender<crate::model::Command>>,
    /// Registry-supplied owner and engine fence for this view only.
    pub(crate) endpoint: Option<(ComponentToken, u64)>,
}
impl ViewCx<'_> {
    /// Current spinner glyph, on the shared core animation clock.
    pub fn spinner(&self) -> &'static str { spinner(self.frame) }
    /// Typed widget callback; neither editor handles nor mutable core state escape.
    pub fn callback<M: Send + 'static>(&self, make: impl Fn(String) -> M + Send + Sync + 'static) -> Option<impl Fn(String) + 'static> {
        let commands = self.commands?.clone();
        let (to, engine_epoch) = self.endpoint?;
        Some(move |value| {
            let _ = commands.send(crate::model::Command::ComponentInput(LocalReply {
                to, engine_epoch, body: Box::new(make(value)),
            }));
        })
    }
}
pub(crate) fn spinner(frame: usize) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(frame / 5) % FRAMES.len()]
}
/// A separate input widget; the shell retains its handle and the chat draft.
pub struct EditorConfig {
    /// Current owner-controlled value.
    pub value: String,
    /// Whether this widget receives editing keys.
    pub focused: bool,
    /// Typed owner callback from ViewCx.
    pub on_change: Box<dyn FnMut(String) + Send + Sync>,
}
/// Owned presentation; requested rows are used for both layout and view.
pub enum View {
    /// Plain header/status segment, or non-wrapping slot text.
    Text(String),
    /// Toolkit-owned view with exact geometry.
    Element {
        /// No borrowed model/component state survives rendering.
        element: AnyElement<'static>,
        /// The same row budget core reserves.
        rows: usize,
        /// Replace earlier content in this slot instead of appending.
        replace: bool,
    },
    /// Owner-built chrome around a shell-rendered separate input widget.
    Editor {
        /// Value, focus and typed edit callback, never a mutable handle.
        editor: EditorConfig,
        /// Receives only the rendered input element.
        render: Box<dyn FnOnce(AnyElement<'static>) -> AnyElement<'static>>,
        /// Exact reserved rows.
        rows: usize,
        /// Replace earlier content in this slot.
        replace: bool,
    },
}
/// Serialized by the core stdout owner, before core event reduction.
pub enum Plain {
    /// Stream bytes and flush.
    Text(String),
    /// One newline-terminated line.
    Line(String),
    /// Notice that the requested interaction needs a terminal.
    TtyRequired(String),
    /// Existing query-reply completion predicate, with no printable bytes.
    Finished,
}
/// Borrowed authenticated transcript provenance, independent of a feature DTO.
#[derive(Clone, Copy)]
pub struct WrapperOrigin<'a> {
    /// Stable message identity.
    pub id: uuid::Uuid,
    /// Authenticated sender address.
    pub from: &'a str,
    /// Exact answered message, when present.
    pub reply_to: Option<uuid::Uuid>,
}
/// One parsed injected wrapper or core user/message row.
#[derive(Clone, Copy)]
pub struct Wrapper<'a> {
    /// Protocol tag; the registry indexes its sole owner.
    pub tag: &'a str,
    /// Borrowed opening tag and attributes, without angle brackets.
    pub open: &'a str,
    /// Body with the existing wrapper newline trimming applied.
    pub body: &'a str,
    /// Sender of a direct live message rather than an XML attribute.
    pub sender: Option<&'a str>,
    /// Authenticated history-row provenance, never inferred from wrapper text.
    pub origin: Option<WrapperOrigin<'a>>,
    /// Live echo routing rather than accepted-history reconstruction.
    pub live: bool,
}
/// Owner projection at the existing wrapper position; no registration-order composition.
pub enum WrapperView {
    /// Retain the existing core rendering for this row.
    Fallback,
    /// The row is already represented or deliberately invisible.
    Hidden,
    /// One owner-formatted card.
    Card {
        /// Neutral transcript presentation.
        kind: crate::model::CardKind,
        /// Header.
        title: String,
        /// Body.
        body: String,
    },
}
/// Owned transcript presentation, with no feature DTO in core.
pub struct TranscriptCard {
    /// Newer cards freeze earlier cards in this replacement group.
    pub group: &'static str,
    /// Stable identity within the current transcript, never a Vec index.
    pub key: uuid::Uuid,
    /// Header marker.
    pub glyph: &'static str,
    /// Header color.
    pub color: iocraft::Color,
    /// Card title.
    pub title: String,
    /// Owned rendered body; its width/fence options survive the core append unchanged.
    pub body: markdown::Text,
    /// Initial disclosure state.
    pub expanded: bool,
    /// Render literal cleaned text, without Markdown or terminal hyperlinks.
    pub plain: bool,
}
impl From<TranscriptCard> for crate::model::Entry {
    fn from(card: TranscriptCard) -> Self {
        Self::Card {
            kind: crate::model::CardKind::Component { group: card.group, key: card.key,
                glyph: card.glyph, color: card.color, plain: card.plain },
            title: card.title, text: Box::new(card.body), expanded: card.expanded,
        }
    }
}
/// A core transcript frame built with the authoritative history projection.
pub struct TranscriptPresentation {
    /// Selected session for this frame.
    pub session: SessionId,
    /// Applied session metadata; history has already been projected into entries.
    pub loaded: agent::Loaded,
    /// Owned, already-rendered transcript entries.
    pub entries: Vec<crate::model::Entry>,
    /// First visible entry and row.
    pub anchor: Option<(usize, usize)>,
    /// Keep live tail stitching and composer input out of this frame.
    pub read_only: bool,
}
/// Core-only presentation operations, applied after registry borrows end.
pub enum CoreAction {
    /// Append a core notice.
    Notice(String),
    /// Insert above a streaming tail using the existing transcript policy.
    Interject(String),
    /// A local validation refusal, not a failed asynchronous command.
    Reject(String),
    /// Existing generic error semantics, including print-mode completion.
    Error(String),
    /// Invoke the existing named skill path, including draft-session adoption.
    UseSkill {
        /// Catalog skill name.
        name: String,
        /// User-supplied arguments.
        args: String,
    },
    /// Append an owner-formatted card; this becomes the group's current card.
    AppendCard(TranscriptCard),
    /// Replace only the still-current card, leaving superseded cards frozen.
    ReplaceCard {
        /// Named replacement group.
        group: &'static str,
        /// Stable card identity.
        key: uuid::Uuid,
        /// New markdown body.
        body: String,
    },

    // Step-11 group t1 CoreAction variants: add below, in this block only.
    /// Insert an owner-rendered card above a still-streaming tail without splitting it.
    InterjectCard(TranscriptCard),
    /// Navigate through the existing core session/ownership policy.
    ResumeSession(SessionId),

    // Step-11 group t2 CoreAction variants: add below, in this block only.

    /// Apply authoritative revision/editability metadata without retaining an inventory.
    ContextState {
        /// Selected session whose context was read.
        session: SessionId,
        /// Revision required by a future core edit transaction.
        revision: String,
        /// Whether the authoritative snapshot permits edits.
        editable: bool,
        /// Refusal reason; retained only for a non-editable snapshot.
        reason: Option<String>,
    },

    /// Run the core context mutation and its settled reload as one transaction.
    EditContext {
        /// Session whose authoritative inventory supplied the targets.
        session: SessionId,
        /// Revision reviewed by the user.
        expected_revision: String,
        /// Exact core transcript targets, including protocol companions.
        targets: Vec<agent::ContextTarget>,
    },

    // Step-11 group t3 CoreAction variants: add below, in this block only.
    /// Move the shell-owned separate editor cursor without touching the chat draft.
    EditorCursor(usize),
    /// Hold a core reference and prepend its token to the composer.
    HoldReference(agent::attach::AttachRef),
    /// Scroll the core transcript at the caller's measured geometry.
    Scroll {
        /// Signed row displacement.
        delta: isize,
        /// Visible transcript row budget.
        rows: usize,
        /// Measured terminal columns.
        width: usize,
    },
    /// Build after registry mutation ends, present the frame, then deliver its typed receipt.
    PresentTranscript(Box<dyn FnOnce(&Registry) -> (TranscriptPresentation, LocalReply) + Send>),
    /// Change only the core historical-frame input/admission mode.
    SetReadOnly(bool),
    /// Deliver owner input synchronously after preceding core operations.
    Post(LocalReply),
    /// Adopt one core-created draft before releasing queued owner work.
    EnsureSession(SessionContinuation),
}

/// Owned effects and actions emitted during one deterministic dispatch.
#[derive(Default)]
pub struct Outbox {
    /// Deferred core mutations.
    pub actions: Vec<CoreAction>,
    /// Executor tasks, independent of feature RPC contracts.
    pub effects: Vec<Effect>,
    /// Pre-event non-TTY output.
    pub plain: Vec<Plain>,
    /// New exclusive input claim, for residual core modal arbitration.
    pub(crate) claimed: Option<FocusClass>,
    consumed: bool,
    focus: Option<(ComponentToken, Option<FocusClass>)>,
    pub(crate) error_claimed: bool,
    pub(crate) plain_claimed: bool,
}
/// Typed ports; the core view is read-only and mutations are deferred.
pub struct Cx<'a, 'core, M> {
    /// Borrowed core state before the event.
    pub core: &'a CoreView<'core>,
    token: ComponentToken,
    epoch: u64,
    out: &'a mut Outbox,
    message: std::marker::PhantomData<M>,
}
impl Outbox {
    /// A handled key must stop at this recipient.
    pub fn consumed(&self) -> bool { self.consumed }
}
impl<M: Send + 'static> Cx<'_, '_, M> {
    /// Schedule a typed completion, fenced by slot and engine epoch.
    pub fn spawn<F, Fut>(&mut self, lane: Lane, work: F)
    where F: FnOnce(Arc<Engine>) -> Fut + Send + 'static,
          Fut: Future<Output = M> + Send + 'static,
    {
        let (to, engine_epoch) = (self.token, self.epoch);
        self.out.effects.push(Effect {
            #[cfg(test)]
            request: None,
            lane, work: Box::new(move |engine| Box::pin(async move {
                LocalReply { to, engine_epoch, body: Box::new(work(engine).await) }
            }))
        });
    }
    /// Carry an owner-typed request into its effect; tests can inspect the same value.
    pub fn spawn_request<R, F, Fut>(&mut self, lane: Lane, request: R, work: F)
    where R: Clone + Send + 'static,
          F: FnOnce(Arc<Engine>, R) -> Fut + Send + 'static,
          Fut: Future<Output = M> + Send + 'static,
    {
        #[cfg(test)]
        let observed = request.clone();
        self.spawn(lane, move |engine| work(engine, request));
        #[cfg(test)]
        { self.out.effects.last_mut().unwrap().request = Some(Box::new(observed)); }
    }
    /// Stop propagation of this key/paste/submit.
    pub fn consume_key(&mut self) { self.out.consumed = true; }
    /// Exclusively focus this component at an existing input boundary.
    pub fn focus(&mut self, class: FocusClass) { self.out.focus = Some((self.token, Some(class))); }
    /// Release this component's focus without discarding its cache.
    pub fn close(&mut self) { self.out.focus = Some((self.token, None)); }
    /// Queue a core-only operation.
    pub fn core(&mut self, action: CoreAction) { self.out.actions.push(action); }
    /// Keep transcript interjection ordering under core control.
    pub fn interject(&mut self, text: String) { self.core(CoreAction::Interject(text)); }
    /// Retain generic-error side effects instead of isolating requests silently.
    pub fn error(&mut self, text: String) { self.core(CoreAction::Error(text)); }
    /// Format against pre-event state before mutating component state.
    pub fn plain(&mut self, text: Plain) { self.out.plain.push(text); }

    // Step-11 group t1 Cx helpers: add below, in this block only.

    // Step-11 group t2 Cx helpers: add below, in this block only.

    // Step-11 group t3 Cx helpers: add below, in this block only.
    /// Project through the same registry/history helper as ordinary core reloads.
    pub fn present_transcript<F>(&mut self, build: F)
    where F: FnOnce(&Registry) -> (TranscriptPresentation, M) + Send + 'static,
    {
        let (to, engine_epoch) = (self.token, self.epoch);
        self.core(CoreAction::PresentTranscript(Box::new(move |registry| {
            let (presentation, message) = build(registry);
            (presentation, LocalReply { to, engine_epoch, body: Box::new(message) })
        })));
    }
    /// Continue after queued core mutations, retaining the slot/engine fence.
    pub fn post(&mut self, message: M) {
        self.core(CoreAction::Post(LocalReply {
            to: self.token, engine_epoch: self.epoch, body: Box::new(message),
        }));
    }
    /// Keep an error inline after the core's existing generic failure transitions.
    pub fn claim_error(&mut self) { self.out.error_claimed = true; }
    /// Own this event's plain output without bypassing core admission/bookkeeping.
    pub fn claim_plain(&mut self) { self.out.plain_claimed = true; }
    /// Run owner work after core adopts a session, sharing any creation in flight.
    pub fn ensure_session<F, Fut, E>(&mut self, work: F, on_fail: E)
    where F: FnOnce(Arc<Engine>, SessionId) -> Fut + Send + 'static,
          Fut: Future<Output = M> + Send + 'static,
          E: FnOnce(String) -> M + Send + 'static,
    {
        self.ensure_session_request((), move |engine, session, ()| work(engine, session), on_fail);
    }
    /// Carry the owner's typed request on the effect released after session adoption.
    pub fn ensure_session_request<R, F, Fut, E>(&mut self, request: R, work: F, on_fail: E)
    where R: Clone + Send + 'static,
          F: FnOnce(Arc<Engine>, SessionId, R) -> Fut + Send + 'static,
          Fut: Future<Output = M> + Send + 'static,
          E: FnOnce(String) -> M + Send + 'static,
    {
        let (to, engine_epoch) = (self.token, self.epoch);
        self.core(CoreAction::EnsureSession(SessionContinuation {
            work: Box::new(move |session| {
                #[cfg(test)]
                let observed = request.clone();
                Effect {
                    lane: Lane::Concurrent,
                    #[cfg(test)]
                    request: Some(Box::new(observed)),
                    work: Box::new(move |engine| Box::pin(async move {
                        LocalReply { to, engine_epoch, body: Box::new(work(engine, session, request).await) }
                    })),
                }
            }),
            fail: Box::new(move |error| LocalReply { to, engine_epoch, body: Box::new(on_fail(error)) }),
        }));
    }
}

/// Synchronous owner-local state machine, erased only by Mounted<C>.
pub trait Component: Send + Sync + 'static {
    /// This owner's wire/local-reply message, never UiEvent.
    type Msg: Send + 'static;
    /// Static id, interests, commands and bindings.
    fn declaration(&self) -> Declaration;
    /// Borrow original JSON; unknown versions/variants return None.
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Self::Msg>;
    /// Fold input and queue effects/actions without borrowing mutable Model.
    fn update(&mut self, input: Input<'_, Self::Msg>, cx: &mut Cx<'_, '_, Self::Msg>);
    /// Return owned presentation for a fixed layout slot.
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View>;
    /// Composer-backed prompt configuration for the focused owner.
    fn editor(&self) -> Option<Editor> { None }
    /// Exact additional rows for an owner decoration; shared by layout and view.
    fn entry_rows(&self, _key: &str) -> usize { 0 }
    /// Next Unix-millisecond deadline; idle components do not wake the loop.
    fn wake(&self) -> Option<u64> { None }

    // Step-11 group t1 Component methods (with defaults): add below, in this block only.
    /// Exclusively owned protocol tags; duplicate registrations are rejected.
    fn wrapper_tags(&self) -> &'static [&'static str] { &[] }
    /// Project one borrowed wrapper for accepted history and live rows alike.
    fn wrapper(&self, _row: Wrapper<'_>) -> WrapperView { WrapperView::Fallback }

    // Step-11 group t2 Component methods (with defaults): add below, in this block only.
    /// Total replacement height at this width; None keeps the core/raw entry.
    fn entry_height(&self, _key: &str, _width: usize) -> Option<usize> { None }
    /// This owner can open a local representation of the stable entry.
    fn entry_openable(&self, _key: &str) -> bool { false }
    /// Keep a command routable while omitting it from help/completion.
    fn slash_visible(&self, _name: &str) -> bool { true }

    // Step-11 group t3 Component methods (with defaults): add below, in this block only.
    /// Intrinsic capture precedes exclusive focus and bound keys at this class.
    fn captures(&self, _class: FocusClass) -> bool { false }
    /// Request core animation frames while an owner operation remains active.
    fn animating(&self) -> bool { false }
    /// The focused view owns a separate widget, not the composer-backed Editor.
    fn separate_editor(&self) -> bool { false }
}

trait ErasedComponent: Send + Sync {
    fn declaration(&self) -> Declaration;
    fn wire(&mut self, event: &TopicEvent, core: &CoreView<'_>, token: ComponentToken, epoch: u64, out: &mut Outbox);
    fn reply(&mut self, reply: LocalReply, core: &CoreView<'_>, out: &mut Outbox);
    fn input(&mut self, input: Input<'_, ()>, core: &CoreView<'_>, token: ComponentToken, epoch: u64, out: &mut Outbox);
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View>;
    fn editor(&self) -> Option<Editor>;
    fn captures(&self, class: FocusClass) -> bool;
    fn wrapper(&self, row: Wrapper<'_>) -> WrapperView;
    fn wake(&self) -> Option<u64>;
    fn animating(&self) -> bool;
    fn separate_editor(&self) -> bool;
    fn entry_rows(&self, key: &str) -> usize;
    fn entry_height(&self, key: &str, width: usize) -> Option<usize>;
    fn entry_openable(&self, key: &str) -> bool;
    fn slash_visible(&self, name: &str) -> bool;
    fn as_any(&self) -> &dyn Any;
    #[cfg(test)]
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
struct Mounted<C>(C);
impl<C: Component> ErasedComponent for Mounted<C> {
    fn declaration(&self) -> Declaration { self.0.declaration() }
    fn wire(&mut self, event: &TopicEvent, core: &CoreView<'_>, token: ComponentToken, epoch: u64, out: &mut Outbox) {
        if let Some(message) = self.0.decode(event, core) {
            self.0.update(Input::Message(message), &mut Cx { core, token, epoch, out, message: std::marker::PhantomData });
        }
    }
    fn reply(&mut self, reply: LocalReply, core: &CoreView<'_>, out: &mut Outbox) {
        if let Ok(message) = reply.body.downcast::<C::Msg>() {
            self.0.update(Input::Message(*message), &mut Cx { core, token: reply.to, epoch: reply.engine_epoch, out, message: std::marker::PhantomData });
        }
    }
    fn input(&mut self, input: Input<'_, ()>, core: &CoreView<'_>, token: ComponentToken, epoch: u64, out: &mut Outbox) {
        if let Some(input) = input.without_message() {
            self.0.update(input, &mut Cx { core, token, epoch, out, message: std::marker::PhantomData });
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> { self.0.view(at, cx) }
    fn editor(&self) -> Option<Editor> { self.0.editor() }
    fn captures(&self, class: FocusClass) -> bool { self.0.captures(class) }
    fn wrapper(&self, row: Wrapper<'_>) -> WrapperView { self.0.wrapper(row) }
    fn wake(&self) -> Option<u64> { self.0.wake() }
    fn animating(&self) -> bool { self.0.animating() }
    fn separate_editor(&self) -> bool { self.0.separate_editor() }
    fn entry_rows(&self, key: &str) -> usize { self.0.entry_rows(key) }
    fn entry_height(&self, key: &str, width: usize) -> Option<usize> { self.0.entry_height(key, width) }
    fn entry_openable(&self, key: &str) -> bool { self.0.entry_openable(key) }
    fn slash_visible(&self, name: &str) -> bool { self.0.slash_visible(name) }
    fn as_any(&self) -> &dyn Any { &self.0 }
    #[cfg(test)]
    fn as_any_mut(&mut self) -> &mut dyn Any { &mut self.0 }
}

/// Fixed generic effect envelope, never a feature-specific Command or reply.
pub async fn execute(effect: Effect, engine: Arc<Engine>) -> UiEvent {
    UiEvent::Component((effect.work)(engine).await)
}
