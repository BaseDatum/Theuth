use super::*;

/// Visible slice of a list `max` rows tall that keeps `cursor` in view.
pub fn window(len: usize, cursor: usize, max: usize) -> Range<usize> {
    let start = (cursor + 1)
        .saturating_sub(max)
        .min(len.saturating_sub(max));
    start..len.min(start + max)
}

/// An open menu; modal except `Slash`, which rides on the input box.
#[derive(Debug)]
pub enum Menu {
    /// Command list filtered by the input's first word.
    Slash {
        /// Highlighted match.
        cursor: usize,
    },
    /// `/model` picker (124 §3): providers level, a provider's models, or word search.
    Models {
        /// Everything `ListModels` returned, in catalog order (provider, id).
        items: Vec<ModelInfo>,
        /// Search text typed inline; non-empty means search mode over every model.
        filter: String,
        /// Highlighted row of `models_rows()`.
        cursor: usize,
        /// The provider whose models are listed; `None` is the providers level.
        provider: Option<String>,
        /// Whose model a pick sets (125 §7.2): the session's or its advisor's.
        target: PickTarget,
    },
    /// `/effort` picker.
    Efforts {
        /// The model's effort levels, then `clear`.
        items: Vec<String>,
        /// Highlighted row.
        cursor: usize,
        /// Whose effort a pick sets.
        target: PickTarget,
    },
    /// `/aws` picker.
    AwsProfiles {
        /// The harness's profiles and selection; row 0 is the default chain.
        info: AwsProfiles,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/sessions` picker.
    Sessions {
        /// Project headings, then roots with their children under them (51 §3).
        items: Vec<Grouped<(SessionSummary, usize)>>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/search` (49 §6): the live menu over [`Model::search`].
    Search,
    /// `/attach` (66 §10): the search menu picking a reference for the input box.
    Attach,
    /// `/agents` picker.
    Agents {
        /// The current session's subtree: running first, then awaiting /
        /// approval, then the rest newest-finished first.
        items: Vec<AgentRow>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/skills` picker.
    Skills {
        /// Every skill, as indexed.
        items: Vec<SkillMeta>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/login` picker.
    Logins {
        /// Providers then OAuth MCP servers.
        items: Vec<LoginRow>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/acp`: configured harnesses, then the registry.
    Acp {
        /// Latest `ListAcp` reply.
        entries: Vec<EntryInfo>,
        /// Latest `ListAcpRegistry` reply; empty until it arrives.
        registry: Vec<RegistryAgent>,
        /// Highlighted row: entries first, then registry rows.
        cursor: usize,
    },
    /// `/edit`, `/regen`, `/fork` picker over the transcript's turns (29 §6.2).
    Turns {
        /// `(entry index, turn seq, first line of the opener)`, oldest first.
        items: Vec<(usize, i64, String)>,
        /// Highlighted row; starts on the newest.
        cursor: usize,
        /// What ⏎ does with the highlighted turn.
        action: TurnAction,
    },
    /// `/artifacts` picker (35 §6.2); `/apps` when `apps` (46 §8.2).
    Artifacts {
        /// Project and session headings, then the rows by name and version (51 §3).
        items: Vec<Grouped<artifact::Artifact>>,
        /// Highlighted row.
        cursor: usize,
        /// Rows are apps: the title says so and where they run.
        apps: bool,
    },
    /// `/shake` picker and review (48 §2.4).
    Shake(Box<Shake>),
}

/// A picker row that may be a heading: the cursor skips headings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grouped<T> {
    /// Non-selectable label.
    Heading(String),
    /// A selectable row.
    Item(T),
}

impl<T> Grouped<T> {
    /// The item, if this is one.
    pub fn item(&self) -> Option<&T> {
        match self {
            Grouped::Item(t) => Some(t),
            Grouped::Heading(_) => None,
        }
    }
}

/// Cursor after moving `delta` selectable rows from `cursor` (headings skipped, ends clamp).
pub fn grouped_step<T>(rows: &[Grouped<T>], cursor: usize, delta: isize) -> usize {
    let mut at = cursor.min(rows.len().saturating_sub(1));
    for _ in 0..delta.unsigned_abs() {
        let next = if delta < 0 {
            (0..at).rev().find(|&i| rows[i].item().is_some())
        } else {
            (at + 1..rows.len()).find(|&i| rows[i].item().is_some())
        };
        match next {
            Some(i) => at = i,
            None => break,
        }
    }
    at
}

/// First selectable row (`0` when there is none).
pub fn grouped_first<T>(rows: &[Grouped<T>]) -> usize {
    rows.iter().position(|r| r.item().is_some()).unwrap_or(0)
}

/// What the key handler should do after a menu saw a key.
#[derive(Debug, PartialEq, Eq)]
pub enum MenuKey {
    /// Nothing more.
    Consumed,
    /// Replace the input text.
    Input(String),
}

impl Model {

    /// Route a key to the open menu (no-op without one). Returns what the
    /// input box should do.
    pub fn menu_key(
        &mut self,
        code: KeyCode,
        input: &str,
        tx: &UnboundedSender<Command>,
    ) -> MenuKey {
        match &self.menu {
            None => MenuKey::Consumed,
            Some(Menu::Slash { .. }) => self.slash_key(code, input, tx),
            Some(Menu::Models { .. }) => self.models_key(code, tx),
            Some(Menu::Efforts { .. }) => self.efforts_key(code, tx),
            Some(Menu::AwsProfiles { .. }) => self.aws_key(code, tx),
            Some(Menu::Sessions { .. }) => self.sessions_key(code, tx),
            Some(Menu::Search | Menu::Attach) => MenuKey::Consumed,
            Some(Menu::Agents { .. }) => self.agents_key(code, tx),
            Some(Menu::Acp { .. }) => self.acp_key(code, tx),
            Some(Menu::Skills { .. }) => self.skills_key(code, tx),
            Some(Menu::Logins { .. }) => self.logins_key(code, tx),
            Some(Menu::Turns { .. }) => self.turns_key(code, tx),
            Some(Menu::Artifacts { .. }) => self.artifacts_key(code, tx),
            Some(Menu::Shake(_)) => self.shake_key(code, tx),
        }
    }
}
