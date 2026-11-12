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
    /// `/mcp` switches and risk lines.
    Mcp {
        /// Latest `ListServers` reply.
        servers: Vec<McpServer>,
        /// Highlighted row of [`Model::mcp_rows`].
        cursor: usize,
        /// Servers whose tools are listed.
        expanded: BTreeSet<String>,
        /// ⏎ on an `on` row: waiting for the scope (`s` session / `a` always).
        pending: Option<Target>,
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
    /// `/nodes`: paired nodes over [`Model::nodes`] (32 §6.7).
    Nodes {
        /// Highlighted node.
        cursor: usize,
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
    /// `/secrets`: the vault's names over [`Model::secrets`] (37 §5).
    Secrets {
        /// Highlighted secret.
        cursor: usize,
        /// History of the highlighted secret is open: the highlighted
        /// version row (0 = current). `None` shows the names.
        history: Option<usize>,
    },
    /// `/knowledge [query]` picker (10 §6).
    Knowledge {
        /// The list or search reply.
        items: Vec<Meta>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/knowledge history [id]`: commits newest first, then one commit's
    /// diff against its parent. Opened when asked, filled by the reply that
    /// matches `id` and `limit`; later or stale replies are dropped.
    KnowledgeHistory {
        /// Concept id, `None` for the whole bundle.
        id: Option<String>,
        /// Shown commits, at most `limit`.
        changes: Vec<Change>,
        /// Page size asked for; `⏎` on the `older` row asks for 20 more.
        limit: usize,
        /// Commits exist past `changes`: the `older` row shows.
        more: bool,
        /// Highlighted commit (or the `older` row at `changes.len()`).
        cursor: usize,
        /// A page request is in flight.
        loading: bool,
        /// Commit whose revision is being fetched; cleared by moving.
        pending: Option<String>,
        /// The last failed request's message, shown in place of the rows.
        error: Option<String>,
        /// Open diff of the highlighted commit: first shown row, first shown column.
        revision: Option<(Revision, usize, usize)>,
    },
    /// `/shake` picker and review (48 §2.4).
    Shake(Box<Shake>),
    /// `/broadcast` picker, composer and review (62 §5.1).
    Broadcast(Box<Broadcast>),
    /// `/peers` (50 §5.1, §5.3): the directory `session` may address, then
    /// the requests a human may resolve for it by exact id. Opened at once,
    /// filled by the replies that name `session`; a switch drops the rest.
    Peers {
        /// Whose view.
        session: SessionId,
        /// `list_peers`: `None` until it arrives; `Err` is the blocker (communication off).
        peers: Option<Result<PeerPage, String>>,
        /// `list_peer_messages(pending_only)`; `None` until it arrives.
        requests: Option<PeerMessagePage>,
        /// Highlighted row of [`Model::peers_rows`].
        cursor: usize,
    },
    /// `/cost`: the shown session's owner summary (51 §6.5, §8.3).
    Cost {
        /// The summary at open (or the latest refresh).
        cost: SessionCost,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/terminals` picker (51 §7.4).
    Terminals {
        /// PTY jobs of the shown session's tree.
        items: Vec<TerminalSummary>,
        /// Highlighted row.
        cursor: usize,
    },
    /// The read-only terminal observer (51 §7.4); drawn by `terminal_box`, never `menu`.
    Terminal(Box<TerminalView>),
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
            Some(Menu::Mcp { .. }) => self.mcp_key(code, tx),
            Some(Menu::Acp { .. }) => self.acp_key(code, tx),
            Some(Menu::Skills { .. }) => self.skills_key(code, tx),
            Some(Menu::Logins { .. }) => self.logins_key(code, tx),
            Some(Menu::Turns { .. }) => self.turns_key(code, tx),
            Some(Menu::Nodes { .. }) => self.nodes_key(code, tx),
            Some(Menu::Secrets { .. }) => self.secrets_key(code, tx),
            Some(Menu::Artifacts { .. }) => self.artifacts_key(code, tx),
            Some(Menu::Shake(_)) => self.shake_key(code, tx),
            Some(Menu::Broadcast(_)) => self.broadcast_key(code, tx),
            Some(Menu::Peers { .. }) => self.peers_key(code, tx),
            Some(Menu::Knowledge { .. }) => self.knowledge_key(code, tx),
            Some(Menu::KnowledgeHistory { .. }) => self.knowledge_history_key(code, tx),
            Some(Menu::Cost { .. }) => self.cost_key(code),
            Some(Menu::Terminals { .. }) => self.terminals_key(code, tx),
            Some(Menu::Terminal(_)) => self.terminal_key(code, tx),
        }
    }
}
