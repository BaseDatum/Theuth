//! 201 §11 (P6b): tool-list stability in an endless chat. The direct tool
//! definitions and the `## Runtime` index sit at the top of the request, so
//! any change to them breaks the cache for everything after. A [`ToolPin`]
//! freezes both per generation: changes that arrive mid-generation wait for
//! the next trim (or a cold cache) — new tools are announced after the
//! turn's input and callable through the runtime at once; a tool gone from
//! the registry stays defined and its call refuses. A per-session hide (the
//! tool still registered: a hide by name, MCP server or category, or 206's
//! harness-wide builtin switch) applies at once and is told once by the same
//! note; switched back on, it is an addition again.

use std::collections::BTreeSet;

use genai::chat::Tool;

/// At most this many names in the switch-off sentence; the rest are counted.
const OFF_NAMES: usize = 12;

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
    /// 206: names hidden since a rebase saw them that no `<tools>` note told
    /// of yet.
    pub switched_off: BTreeSet<String>,
    /// 206: names a `<tools>` note told were switched off, still off.
    pub told_off: BTreeSet<String>,
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
            switched_off: BTreeSet::new(),
            told_off: BTreeSet::new(),
            pending: false,
        }
    }

    /// A name the pin carries or announced is missing from `now`: the caller
    /// probes the unfiltered registry list to tell a hide from a removal.
    pub fn lost(&self, now: &BTreeSet<String>) -> bool {
        self.names
            .iter()
            .chain(&self.announced)
            .any(|n| !now.contains(n))
    }

    /// Fold one rebase's fresh list into the pin. A name gone from `fresh`
    /// is a hide when `registered(name)` (the tool still exists for others):
    /// it leaves the pin now and the index is taken fresh; otherwise it is a
    /// removal and stays defined (its call refuses) until the rebuild. A
    /// hidden name is forgotten as announced (switched back on, it is
    /// announced again) and waits for the switch-off note.
    /// Additions never enter the pin. Returns whether the pinned index text
    /// changed (a hide).
    pub fn reconcile(&mut self, fresh: &Fresh<'_>, registered: impl Fn(&str) -> bool) -> bool {
        let gone: Vec<String> = self
            .names
            .iter()
            .chain(&self.announced)
            .filter(|n| !fresh.names.contains(*n))
            .cloned()
            .collect();
        let mut hidden = false;
        for name in gone {
            if registered(&name) {
                if self.names.remove(&name) {
                    self.tools.retain(|t| t.name.as_str() != name);
                    hidden = true;
                }
                self.announced.remove(&name);
                if !self.told_off.contains(&name) {
                    self.switched_off.insert(name);
                }
            } else if self.names.contains(&name) {
                self.removed.insert(name);
            }
        }
        // A removed name that came back is defined again; a hidden one that
        // came back is no longer off.
        self.removed.retain(|n| !fresh.names.contains(n));
        self.switched_off.retain(|n| !fresh.names.contains(n));
        self.told_off.retain(|n| !fresh.names.contains(n));
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

    /// 206: the names to tell as switched off now, sorted: those a rebase saw
    /// hidden and those the pin carries or announced that are gone from
    /// `session` (the session's registry list) while `registered(name)`,
    /// minus any told already; they count as told and are forgotten as
    /// announced. A name back in `session` before its note is dropped.
    pub fn switch_offs(
        &mut self,
        session: &BTreeSet<String>,
        registered: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        self.told_off.retain(|n| !session.contains(n));
        let mut off = std::mem::take(&mut self.switched_off);
        off.extend(
            self.names
                .iter()
                .chain(&self.announced)
                .filter(|n| !session.contains(*n) && registered(n))
                .cloned(),
        );
        off.retain(|n| !session.contains(n) && !self.told_off.contains(n));
        for n in &off {
            self.announced.remove(n);
        }
        self.told_off.extend(off.iter().cloned());
        off.into_iter().collect()
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

/// 206: the switch-off sentence of the `<tools>` note for `names` (sorted):
/// the first [`OFF_NAMES`], then how many more.
pub fn switched_off_line(names: &[String]) -> String {
    let shown = names[..names.len().min(OFF_NAMES)].join(", ");
    let more = names.len().saturating_sub(OFF_NAMES);
    let list = if more > 0 {
        format!("{shown} and {more} more")
    } else {
        shown
    };
    format!(
        "Switched off for this chat (calls are refused until they are switched back on): {list}"
    )
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
        // The hide is told once, the removal never.
        assert_eq!(pin.switch_offs(&now, |n| n == "c"), ["c"]);
        assert!(pin.switch_offs(&now, |n| n == "c").is_empty(), "told once");
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
        // E-1: the announced `d` switched off is forgotten as announced, so
        // switched on again in this generation it is announced again.
        let off = names(&["a", "b"]);
        let changed = pin.reconcile(
            &Fresh {
                index: "IDX a d",
                tools: &fresh_tools,
                names: &off,
            },
            |n| n == "d",
        );
        assert!(!changed, "an announced name is not in the pinned index");
        assert!(!pin.announced.contains("d"));
        assert_eq!(pin.switch_offs(&off, |_| false), ["d"], "told by a rebase");
        let on = names(&["a", "b", "d"]);
        pin.reconcile(
            &Fresh {
                index: "IDX a d",
                tools: &fresh_tools,
                names: &on,
            },
            |_| true,
        );
        assert_eq!(pin.announce(&on), ["d"], "announced again");
        assert!(
            !pin.told_off.contains("d"),
            "switched on: a later switch-off is told again"
        );
    }

    #[test]
    fn a_switch_off_seen_before_the_rebase_is_told_once() {
        let base = names(&["a", "b", "python"]);
        let tools = vec![tool("a"), tool("python")];
        let fresh = Fresh {
            index: "IDX b",
            tools: &tools,
            names: &base,
        };
        let mut pin = ToolPin::new(1, &fresh);
        // The session's registry list (no loop-local `python`) lacks `a`.
        let session = names(&["b"]);
        assert!(pin.lost(&session));
        let registered = |n: &str| n == "a" || n == "b";
        assert_eq!(pin.switch_offs(&session, registered), ["a"]);
        // The rebase applies the hide; nothing is told twice.
        let now = names(&["b", "python"]);
        let fresh_tools = vec![tool("python")];
        pin.reconcile(
            &Fresh {
                index: "IDX b",
                tools: &fresh_tools,
                names: &now,
            },
            registered,
        );
        assert!(pin.switched_off.is_empty());
        assert!(pin.switch_offs(&session, registered).is_empty());
        // Off and on again before the note: dropped.
        pin.reconcile(
            &Fresh {
                index: "IDX",
                tools: &fresh_tools,
                names: &names(&["python"]),
            },
            registered,
        );
        assert!(pin.switched_off.contains("b"));
        assert!(pin.switch_offs(&names(&["a", "b"]), registered).is_empty());
    }

    #[test]
    fn the_switch_off_line_counts_past_twelve() {
        let few = vec!["grep".to_owned(), "read_file".to_owned()];
        assert_eq!(
            switched_off_line(&few),
            "Switched off for this chat (calls are refused until they are switched back on): grep, read_file"
        );
        let many: Vec<String> = (0..15).map(|i| format!("t{i:02}")).collect();
        let line = switched_off_line(&many);
        assert!(line.ends_with("t10, t11 and 3 more"), "{line}");
        assert!(!line.contains("t12"));
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
        assert!(pin.switch_offs(&n, |_| true).is_empty());
    }
}
