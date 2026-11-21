//! 201 §11 (P6b): tool-list stability in an endless chat. The direct tool
//! definitions and the `## Runtime` index sit at the top of the request, so
//! any change to them breaks the cache for everything after. A [`ToolPin`]
//! freezes both per generation: changes that arrive mid-generation wait for
//! the next trim (or a cold cache) — new tools are announced after the
//! turn's input and callable through the runtime at once; a tool gone from
//! the registry stays defined and its call refuses. A per-session hide (the
//! tool still registered) applies at once.

use std::collections::BTreeSet;

use genai::chat::Tool;

/// The tool part of the request prefix, frozen for one generation.
#[derive(Clone)]
pub struct ToolPin {
    /// The generation it was frozen in.
    pub generation: u32,
    /// The `## Runtime` index block as sent.
    pub index: String,
    /// The direct tool definitions as sent.
    pub tools: Vec<Tool>,
    /// Every name the prefix carries (the session's registry list and the
    /// direct tools).
    pub names: BTreeSet<String>,
    /// Names added since the pin that a `<tools>` note announced already.
    pub announced: BTreeSet<String>,
    /// Names gone from the registry since the pin: still defined, refused.
    pub removed: BTreeSet<String>,
    /// A change waits for the next rebuild (status).
    pub pending: bool,
}

/// One rebase's freshly computed tool part.
pub struct Fresh<'a> {
    /// The `## Runtime` index block as computed now.
    pub index: &'a str,
    /// The direct tool definitions as computed now.
    pub tools: &'a [Tool],
    /// The session's registry names and the direct tools' names.
    pub names: &'a BTreeSet<String>,
}

impl ToolPin {
    /// Freeze `fresh` for `generation`.
    pub fn new(generation: u32, fresh: &Fresh<'_>) -> Self {
        Self {
            generation,
            index: fresh.index.to_owned(),
            tools: fresh.tools.to_vec(),
            names: fresh.names.clone(),
            announced: BTreeSet::new(),
            removed: BTreeSet::new(),
            pending: false,
        }
    }

    /// Fold one rebase's fresh list into the pin. A name gone from `fresh`
    /// is a hide when `registered(name)` (the tool still exists for others):
    /// it leaves the pin now and the index is taken fresh; otherwise it is a
    /// removal and stays defined (its call refuses) until the rebuild.
    /// Additions never enter the pin. Returns whether the pinned index text
    /// changed (a hide).
    pub fn reconcile(&mut self, fresh: &Fresh<'_>, registered: impl Fn(&str) -> bool) -> bool {
        let gone: Vec<String> = self.names.difference(fresh.names).cloned().collect();
        let mut hidden = false;
        for name in gone {
            if registered(&name) {
                self.names.remove(&name);
                self.tools.retain(|t| t.name.as_str() != name);
                hidden = true;
            } else {
                self.removed.insert(name);
            }
        }
        // A removed name that came back is defined again.
        self.removed.retain(|n| !fresh.names.contains(n));
        let index_changed = hidden && self.index != fresh.index;
        if index_changed {
            self.index = fresh.index.to_owned();
        }
        self.pending = !self.removed.is_empty()
            || fresh.names.difference(&self.names).next().is_some()
            || self.index != fresh.index;
        index_changed
    }

    /// Names in `fresh` the prefix does not carry and no note announced yet;
    /// they are marked announced.
    pub fn announce(&mut self, fresh: &BTreeSet<String>) -> Vec<String> {
        let new: Vec<String> = fresh
            .difference(&self.names)
            .filter(|n| !self.announced.contains(*n))
            .cloned()
            .collect();
        self.announced.extend(new.iter().cloned());
        if !new.is_empty() {
            self.pending = true;
        }
        new
    }

    /// The refusal a call of a removed tool gets, `None` for any other.
    pub fn refusal(&self, name: &str) -> Option<String> {
        self.removed.contains(name).then(|| {
            format!(
                "{name} is no longer available in this session (it was removed; it leaves the tool list at the next memory trim)"
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> Tool {
        Tool::new(name)
    }

    fn names(n: &[&str]) -> BTreeSet<String> {
        n.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn additions_wait_removals_refuse_hides_apply() {
        let base = names(&["a", "b", "c"]);
        let tools = vec![tool("a"), tool("b")];
        let mut pin = ToolPin::new(
            3,
            &Fresh {
                index: "IDX a b c",
                tools: &tools,
                names: &base,
            },
        );
        // `d` added, `b` removed from the registry, `c` hidden for this chat.
        let now = names(&["a", "d"]);
        let fresh_tools = vec![tool("a"), tool("d")];
        let fresh = Fresh {
            index: "IDX a d",
            tools: &fresh_tools,
            names: &now,
        };
        let changed = pin.reconcile(&fresh, |n| n == "c");
        assert!(changed, "a hide takes the fresh index");
        assert_eq!(pin.index, "IDX a d");
        let sent: Vec<&str> = pin.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(sent, ["a", "b"], "the removed `b` stays defined, `d` waits");
        assert!(pin.refusal("b").is_some());
        assert!(pin.refusal("a").is_none());
        assert!(pin.pending);
        assert_eq!(pin.announce(&now), ["d"]);
        assert!(pin.announce(&now).is_empty(), "announced once");
        // `b` comes back: defined again, no refusal.
        let back = names(&["a", "b", "d"]);
        pin.reconcile(
            &Fresh {
                index: "IDX a d",
                tools: &fresh_tools,
                names: &back,
            },
            |_| false,
        );
        assert!(pin.refusal("b").is_none());
    }

    #[test]
    fn an_unchanged_list_is_not_pending() {
        let n = names(&["a"]);
        let tools = vec![tool("a")];
        let fresh = Fresh {
            index: "IDX",
            tools: &tools,
            names: &n,
        };
        let mut pin = ToolPin::new(1, &fresh);
        assert!(!pin.reconcile(&fresh, |_| true));
        assert!(!pin.pending);
        assert!(pin.announce(&n).is_empty());
    }
}
