//! 191 (D2): the contact records as the reconcile reads them. Pure functions over
//! [`Records`], so every machine computes the same result from the same rows:
//! - [`stage_records`]: the contacts and exports this machine admits, with per-record
//!   quarantine (a bad record is listed with its problem and its endpoints denied; the rest
//!   stage);
//! - [`reconcile_writes`]: the converging writes (merge pointers to the group's root, unique
//!   aliases and export names);
//! - [`import`]: a `peers.yaml` of an earlier build turned into records;
//! - [`group_of`] and friends: which rows a contact RPC writes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use agent::store::contacts::{
    Cell, ContactRow, EndpointRow, GrantRow, RecordTable, RecordWrite, Records,
};
use uuid::Uuid;

use crate::config::{
    AgentExport, ContactFacts, Export, FirstContact, Peer, PeersFile, Quarantined, Staged,
    check_alias,
};

/// A merge chain that does not reach its root within this many hops quarantines its records.
pub const CHAIN_MAX: usize = 16;

/// The merge order: the lower `(paired_ms, id)` survives (a record without `paired_ms` last).
fn key(c: &ContactRow) -> (i64, &str) {
    (c.paired_ms.unwrap_or(i64::MAX), c.id.as_str())
}

fn now_ms() -> i64 {
    mesh::identity::now_ms() as i64
}

/// The binding uuid of `peer:<uuid>`.
pub fn binding_id(binding: &str) -> Option<Uuid> {
    binding.strip_prefix("peer:")?.parse().ok()
}

/// A fresh `peer:<uuid>` binding.
pub fn fresh_binding() -> String {
    format!("peer:{}", Uuid::now_v7())
}

/// `ask` (NULL), `allow`, `block` → the answer; anything else reads as `block` (strictest).
fn answer(fc: Option<&str>) -> FirstContact {
    match fc {
        None | Some("ask") => FirstContact::Ask,
        Some("allow") => FirstContact::Allow,
        Some(_) => FirstContact::Block,
    }
}

/// The stored text of an answer.
pub fn answer_text(a: FirstContact) -> &'static str {
    match a {
        FirstContact::Ask => "ask",
        FirstContact::Allow => "allow",
        FirstContact::Block => "block",
    }
}

/// Strictness: `block` > `ask` > `allow`.
fn strictness(a: FirstContact) -> u8 {
    match a {
        FirstContact::Allow => 0,
        FirstContact::Ask => 1,
        FirstContact::Block => 2,
    }
}

/// The records of one merge group: its root (whose `merged_into` is NULL) and every live
/// record that resolves to it, root included.
#[derive(Debug)]
pub struct Group<'a> {
    /// The survivor.
    pub root: &'a ContactRow,
    /// Every live record of the group, root first, then by key.
    pub members: Vec<&'a ContactRow>,
}

impl Group<'_> {
    /// The group's anchors (not barred), by `(paired_ms, eid)`, one per eid.
    fn anchors<'r>(&self, r: &'r Records) -> Vec<&'r EndpointRow> {
        let ids: BTreeSet<&str> = self.members.iter().map(|m| m.id.as_str()).collect();
        let mut out: Vec<&EndpointRow> = r
            .endpoints
            .iter()
            .filter(|e| e.barred_ms.is_none() && e.eid.is_some())
            .filter(|e| e.contact.as_deref().is_some_and(|c| ids.contains(c)))
            .collect();
        out.sort_by(|a, b| (a.paired_ms, &a.eid).cmp(&(b.paired_ms, &b.eid)));
        let mut seen = BTreeSet::new();
        out.retain(|e| seen.insert(e.eid.clone()));
        out
    }

    /// The group's grant rows, by export id.
    fn grants<'r>(&self, r: &'r Records) -> BTreeMap<&'r str, Vec<&'r GrantRow>> {
        let ids: BTreeSet<&str> = self.members.iter().map(|m| m.id.as_str()).collect();
        let mut out: BTreeMap<&str, Vec<&GrantRow>> = BTreeMap::new();
        for g in &r.grants {
            if let (Some(c), Some(x)) = (g.contact.as_deref(), g.export.as_deref())
                && ids.contains(c)
            {
                out.entry(x).or_default().push(g);
            }
        }
        out
    }
}

/// How the records fall apart: live groups, removed records and records whose merge chain
/// is broken (a loop, too long, or a pointer to a missing record).
#[derive(Debug, Default)]
pub struct Analysis<'a> {
    /// Live groups, by their root's key.
    pub groups: Vec<Group<'a>>,
    /// Tombstoned records, and live ones merged into one, with the removal time that applies
    /// (their own, else their root's).
    pub removed: Vec<(&'a ContactRow, i64)>,
    /// Records whose chain is broken, with why; `true` when it is replication lag (a pointer
    /// into a record this machine lacks yet), which never revokes the binding.
    pub broken: Vec<(&'a ContactRow, String, bool)>,
}

/// Why [`root_of`] found no root; `lag` for a pointer into a record this machine lacks.
struct Broken {
    why: String,
    lag: bool,
}

/// The root id of `c`, following `merged_into`.
fn root_of<'a>(
    by_id: &HashMap<&'a str, &'a ContactRow>,
    c: &'a ContactRow,
) -> Result<&'a ContactRow, Broken> {
    let mut at = c;
    let mut seen = BTreeSet::from([c.id.as_str()]);
    for _ in 0..=CHAIN_MAX {
        let Some(next) = at.merged_into.as_deref() else {
            return Ok(at);
        };
        let Some(n) = by_id.get(next) else {
            return Err(Broken {
                why: format!("it was merged into a contact this machine lacks ({next})"),
                lag: true,
            });
        };
        if !seen.insert(n.id.as_str()) {
            return Err(Broken {
                why: "its merge chain loops".into(),
                lag: false,
            });
        }
        at = n;
    }
    Err(Broken {
        why: format!("its merge chain is longer than {CHAIN_MAX}"),
        lag: false,
    })
}

/// Split `r.contacts` into groups, removed and broken records.
pub fn analyze(r: &Records) -> Analysis<'_> {
    let by_id: HashMap<&str, &ContactRow> = r.contacts.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut out = Analysis::default();
    let mut groups: BTreeMap<&str, Vec<&ContactRow>> = BTreeMap::new();
    let mut broken_roots: BTreeMap<&str, (String, bool)> = BTreeMap::new();
    for c in &r.contacts {
        if let Some(at) = c.removed_ms {
            out.removed.push((c, at));
            continue;
        }
        match root_of(&by_id, c) {
            Ok(root) if root.removed_ms.is_some() => {
                out.removed.push((c, root.removed_ms.unwrap_or_default()))
            }
            Ok(root) => groups.entry(root.id.as_str()).or_default().push(c),
            Err(Broken { why, lag }) => {
                out.broken.push((c, why.clone(), lag));
                // The whole group goes: the root the chain does reach, past the bound, too.
                let mut at = c;
                let mut seen = BTreeSet::new();
                while let Some(n) = at.merged_into.as_deref().and_then(|n| by_id.get(n)) {
                    if !seen.insert(n.id.as_str()) {
                        break;
                    }
                    at = n;
                }
                if at.merged_into.is_none() {
                    broken_roots.insert(at.id.as_str(), (why, lag));
                }
            }
        }
    }
    for (root, mut members) in groups {
        if let Some((e, lag)) = broken_roots.get(root) {
            out.broken
                .extend(members.into_iter().map(|m| (m, e.clone(), *lag)));
            continue;
        }
        members.sort_by(|a, b| (a.id != root, key(a)).cmp(&(b.id != root, key(b))));
        out.groups.push(Group {
            root: by_id[root],
            members,
        });
    }
    out.groups.sort_by(|a, b| key(a.root).cmp(&key(b.root)));
    out
}

/// The live group `root` heads (or the group `id` belongs to).
pub fn group_of<'a>(a: &'a Analysis<'a>, id: &str) -> Option<&'a Group<'a>> {
    a.groups
        .iter()
        .find(|g| g.members.iter().any(|m| m.id == id))
}

/// The record id of the live contact (its root) called `alias`.
pub fn root_by_alias<'a>(a: &'a Analysis<'a>, alias: &str) -> Option<&'a Group<'a>> {
    a.groups
        .iter()
        .find(|g| g.root.alias.as_deref() == Some(alias))
}

/// The id of the live, non-barred record whose anchor is `eid`, newest first (an import or a
/// ceremony attaches to it); `removed` too when `with_removed`.
pub fn record_with_eid<'a>(
    r: &'a Records,
    eid: &str,
    with_removed: bool,
) -> Option<&'a ContactRow> {
    let ids: BTreeSet<&str> = r
        .endpoints
        .iter()
        .filter(|e| e.eid.as_deref() == Some(eid))
        .filter_map(|e| e.contact.as_deref())
        .collect();
    r.contacts
        .iter()
        .filter(|c| ids.contains(c.id.as_str()))
        .filter(|c| with_removed || c.removed_ms.is_none())
        .max_by(|a, b| key(a).cmp(&key(b)))
}

/// `base-n` that fits an alias, for the first `n` from 2 not in `taken`.
fn suffixed(base: &str, taken: &BTreeSet<String>) -> String {
    for n in 2.. {
        let tail = format!("-{n}");
        let keep = agent::communication::ALIAS_MAX.saturating_sub(tail.len());
        let head: String = base.chars().take(keep).collect();
        let name = format!("{head}{tail}");
        if !taken.contains(&name) {
            return name;
        }
    }
    unreachable!("an unbounded range")
}

/// The writes that make the records converge (D2): every live record of a set of groups
/// sharing an anchor points at that set's survivor (chains flattened), and the survivors'
/// aliases and the live exports' names are unique (the older keeps it, the newer is renamed
/// `<name>-2`). Every machine computes the same writes from the same rows.
pub fn reconcile_writes(r: &Records) -> Vec<RecordWrite> {
    let a = analyze(r);
    let mut out = Vec::new();
    // Union the groups that share an anchor endpoint.
    let mut parent: Vec<usize> = (0..a.groups.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut by_eid: BTreeMap<String, usize> = BTreeMap::new();
    for (i, g) in a.groups.iter().enumerate() {
        for e in g.anchors(r) {
            let eid = e.eid.clone().unwrap_or_default();
            match by_eid.get(&eid) {
                Some(&j) => {
                    let (x, y) = (find(&mut parent, i), find(&mut parent, j));
                    // Groups are sorted by key: the lower index is the survivor.
                    let (lo, hi) = (x.min(y), x.max(y));
                    parent[hi] = lo;
                }
                None => {
                    by_eid.insert(eid, i);
                }
            }
        }
    }
    let mut survivors = Vec::new();
    for (i, g) in a.groups.iter().enumerate() {
        let s = find(&mut parent, i);
        let target = &a.groups[s].root.id;
        for m in &g.members {
            if m.id != *target && m.merged_into.as_deref() != Some(target.as_str()) {
                out.push(RecordWrite::new(
                    RecordTable::Contacts,
                    m.id.clone(),
                    vec![("merged_into", Cell::Text(target.clone()))],
                ));
            }
        }
        if s == i {
            survivors.push(g.root);
        }
    }
    // Unique aliases among the survivors, in key order.
    let mut taken: BTreeSet<String> = survivors.iter().filter_map(|c| c.alias.clone()).collect();
    let mut kept = BTreeSet::new();
    for c in &survivors {
        let Some(alias) = c.alias.as_deref() else {
            continue;
        };
        if kept.insert(alias.to_owned()) {
            continue;
        }
        let name = suffixed(alias, &taken);
        taken.insert(name.clone());
        kept.insert(name.clone());
        out.push(RecordWrite::new(
            RecordTable::Contacts,
            c.id.clone(),
            vec![("alias", Cell::Text(name))],
        ));
    }
    // Unique export names among the live exports, in id (mint) order.
    let live: Vec<_> = r
        .exports
        .iter()
        .filter(|e| e.removed_ms.is_none() && e.name.is_some())
        .collect();
    let mut taken: BTreeSet<String> = live.iter().filter_map(|e| e.name.clone()).collect();
    let mut kept = BTreeSet::new();
    for e in live {
        let name = e.name.as_deref().unwrap_or_default();
        if kept.insert(name.to_owned()) {
            continue;
        }
        let renamed = suffixed(name, &taken);
        taken.insert(renamed.clone());
        kept.insert(renamed.clone());
        out.push(RecordWrite::new(
            RecordTable::Exports,
            e.id.clone(),
            vec![("name", Cell::Text(renamed))],
        ));
    }
    out
}

/// A group's effective grant of one export (D2 merge): only the rows present count (a
/// record without one abstains); `granted = 0` beats `1`, and the strictest answer wins.
fn effective(rows: &[&GrantRow]) -> (Option<bool>, Option<FirstContact>) {
    let granted = rows
        .iter()
        .filter_map(|g| g.granted)
        .fold(None, |acc: Option<bool>, v| {
            Some(acc.unwrap_or(true) && v != 0)
        });
    let answer = rows
        .iter()
        .map(|g| answer(g.first_contact.as_deref()))
        .max_by_key(|a| strictness(*a));
    (granted, answer)
}

/// Stage the records (D2): exports by name, the live groups as contacts (blocked ones
/// included, without a route), and every record that cannot be proven sound quarantined
/// with its endpoints denied. `family` is this mesh's own machines' endpoint ids.
pub fn stage_records(r: &Records, family: &BTreeSet<String>, contact_cap: tools::Risk) -> Staged {
    let mut staged = Staged::default();
    // Exports: valid live rows, one per name (the oldest id; the reconcile renames the rest).
    let mut export_names: BTreeMap<&str, String> = BTreeMap::new();
    for e in r.exports.iter().filter(|e| e.removed_ms.is_none()) {
        let Some(name) = e.name.as_deref().filter(|n| check_alias(n).is_ok()) else {
            continue;
        };
        if staged.export_ids.contains_key(name) {
            continue;
        }
        let title = e.title.clone().unwrap_or_else(|| name.to_owned());
        let description = e.description.clone().unwrap_or_default();
        match (e.session.as_deref(), e.agent.as_deref()) {
            (Some(s), None) => {
                let Ok(session) = s.parse() else { continue };
                staged.exports.insert(
                    name.to_owned(),
                    Export {
                        name: name.to_owned(),
                        session,
                        title,
                        description,
                        peer: None,
                    },
                );
            }
            (None, Some(agent)) if !agent.trim().is_empty() => {
                staged.agents.insert(
                    name.to_owned(),
                    AgentExport {
                        name: name.to_owned(),
                        agent: agent.trim().to_owned(),
                        title,
                        description,
                    },
                );
            }
            _ => continue,
        }
        staged.export_ids.insert(name.to_owned(), e.id.clone());
        export_names.insert(e.id.as_str(), name.to_owned());
    }
    let export_rows: BTreeSet<&str> = r.exports.iter().map(|e| e.id.as_str()).collect();

    let a = analyze(r);
    // How a quarantine treats its endpoints and binding.
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        /// Unsound: endpoints denied, the binding falls out of the routes (revoked).
        Hard,
        /// Replication lag or a convergence in flight: endpoints denied, the binding kept
        /// (its inboxes capped) so the record comes back whole once the rows arrive.
        Lag,
        /// Removed on another machine: never admitted, its endpoints may pair again.
        Removed,
    }
    let quarantine =
        |c: &ContactRow, eids: Vec<String>, problem: String, kind: Kind, s: &mut Staged| {
            if kind == Kind::Removed {
                s.removed.extend(eids.iter().cloned());
            } else {
                s.deny.extend(eids.iter().cloned());
            }
            s.quarantined.push(Quarantined {
                id: c.id.clone(),
                alias: c.alias.clone().unwrap_or_default(),
                eid: eids.first().cloned().unwrap_or_default(),
                machine: c.machine.clone().unwrap_or_default(),
                paired_ms: c.paired_ms.unwrap_or_default().max(0) as u64,
                problem,
                hold: (kind == Kind::Lag)
                    .then(|| c.binding.as_deref().and_then(binding_id))
                    .flatten()
                    .filter(|b| !r.revoked.contains(&b.to_string())),
            });
        };
    for (c, why, lag) in &a.broken {
        let eids = r
            .endpoints
            .iter()
            .filter(|e| e.contact.as_deref() == Some(c.id.as_str()))
            .filter_map(|e| e.eid.clone())
            .collect();
        let kind = if *lag { Kind::Lag } else { Kind::Hard };
        quarantine(c, eids, why.clone(), kind, &mut staged);
    }
    // When each endpoint was last removed with a record (the latest removal wins).
    let mut removed_at: BTreeMap<&str, i64> = BTreeMap::new();
    for (c, at) in &a.removed {
        for e in r
            .endpoints
            .iter()
            .filter(|e| e.contact.as_deref() == Some(c.id.as_str()))
        {
            if let Some(eid) = e.eid.as_deref() {
                let t = removed_at.entry(eid).or_insert(*at);
                *t = (*t).max(*at);
            }
        }
    }
    let stale = |e: &EndpointRow| {
        e.eid
            .as_deref()
            .and_then(|eid| removed_at.get(eid))
            .is_some_and(|at| e.paired_ms.unwrap_or(0) <= *at)
    };
    // An endpoint anchored in two groups that have not merged yet is denied in both.
    let anchors: Vec<Vec<&EndpointRow>> = a.groups.iter().map(|g| g.anchors(r)).collect();
    let mut owners: BTreeMap<&str, usize> = BTreeMap::new();
    for set in &anchors {
        for e in set {
            *owners
                .entry(e.eid.as_deref().unwrap_or_default())
                .or_default() += 1;
        }
    }
    let mut admitted: BTreeSet<String> = BTreeSet::new();
    for (g, set) in a.groups.iter().zip(&anchors) {
        let c = g.root;
        let eids: Vec<String> = set.iter().filter_map(|e| e.eid.clone()).collect();
        // A grant naming an export row this machine lacks (yet) is skipped, never a reason
        // to drop the contact.
        let mut grants = g.grants(r);
        grants.retain(|x, _| export_rows.contains(x));
        let binding = c.binding.as_deref().and_then(binding_id);
        let problem = if c.alias.as_deref().is_none_or(|a| check_alias(a).is_err()) {
            Some(("its name is malformed", Kind::Hard))
        } else if binding.is_none() {
            Some(("its binding is malformed", Kind::Hard))
        } else if binding.is_some_and(|b| r.revoked.contains(&b.to_string())) {
            Some(("its binding was revoked on this machine", Kind::Hard))
        } else if eids.is_empty() {
            Some(("it has no endpoint", Kind::Hard))
        } else if eids.iter().any(|e| e.parse::<iroh::EndpointId>().is_err()) {
            Some(("an endpoint is malformed", Kind::Hard))
        } else if eids.iter().any(|e| family.contains(e)) {
            Some(("now one of your machines", Kind::Hard))
        } else if set.iter().all(|e| stale(e)) {
            // A duplicate made before a removal synced (or re-paired under clock skew): every
            // anchor was removed with another record no newer ceremony followed.
            Some(("removed on another machine", Kind::Removed))
        } else if eids
            .iter()
            .any(|e| owners.get(e.as_str()).copied().unwrap_or(0) > 1)
        {
            Some(("an endpoint is in another contact too (merging)", Kind::Lag))
        } else if c
            .alias
            .as_deref()
            .is_some_and(|a| staged.contacts.contains_key(a))
        {
            Some(("its name is taken by another contact (renaming)", Kind::Lag))
        } else {
            None
        };
        if let Some((p, kind)) = problem {
            quarantine(c, eids, p.to_owned(), kind, &mut staged);
            continue;
        }
        let (Some(alias), Some(binding)) = (c.alias.clone(), binding) else {
            continue;
        };
        admitted.extend(g.members.iter().map(|m| m.id.clone()));
        let blocked = g.members.iter().any(|m| m.blocked.is_some_and(|b| b != 0));
        let mut exports = Vec::new();
        let mut first_contact = BTreeMap::new();
        for (id, rows) in &grants {
            let Some(name) = export_names.get(id) else {
                continue;
            };
            let (granted, ans) = effective(rows);
            if let Some(ans) = ans {
                first_contact.insert(name.clone(), ans);
            }
            if granted != Some(true) || blocked {
                continue;
            }
            // §6.3: one principal per exported chat; a second contact's grant is skipped
            // (the older contact keeps it, on every machine alike).
            if let Some(x) = staged.exports.get_mut(name) {
                if x.peer.is_some() {
                    continue;
                }
                x.peer = Some(alias.clone());
            }
            exports.push(name.clone());
        }
        let eid = eids[0].clone();
        if blocked {
            staged.deny.extend(eids.iter().cloned());
        } else {
            staged.peers.insert(
                alias.clone(),
                Peer {
                    name: alias.clone(),
                    card: None,
                    outbound: None,
                    inbound: None,
                    sessions: Vec::new(),
                    exports,
                    allow_private: false,
                    eid: Some(eid.clone()),
                    record: Some((c.id.clone(), binding)),
                },
            );
        }
        staged.contacts.insert(
            alias.clone(),
            ContactFacts {
                id: c.id.clone(),
                addrs: r
                    .addrs
                    .get(&eid)
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.parse().ok())
                    .collect(),
                eid,
                eids,
                display: c.name.clone().unwrap_or_else(|| alias.clone()),
                machine: c.machine.clone().unwrap_or_default(),
                cap: contact_cap.min(tools::Risk::Mutating),
                first_contact,
                blocked,
                paired_ms: c.paired_ms.unwrap_or_default().max(0) as u64,
            },
        );
    }
    // A removed record's endpoints stay refused until a newer ceremony pins them in a live
    // record (an anchor paired after the removal): a re-pair works, a stale copy never.
    for (eid, removed) in &removed_at {
        let repaired = r.endpoints.iter().any(|p| {
            p.eid.as_deref() == Some(*eid)
                && p.barred_ms.is_none()
                && p.paired_ms.unwrap_or(0) > *removed
                && p.contact.as_deref().is_some_and(|m| admitted.contains(m))
        });
        if !repaired {
            staged.removed.insert((*eid).to_owned());
        }
    }
    staged
}

/// What [`import`] produced.
#[derive(Debug, Default)]
pub struct Import {
    /// The record writes.
    pub writes: Vec<RecordWrite>,
    /// This machine's file export names → their ids (the inbox markers' backfill map).
    pub exports: BTreeMap<String, String>,
    /// Contacts the file names that only a removed record holds: skipped, never revived.
    pub skipped: Vec<String>,
    /// `contact_addrs` rows: endpoint → addresses.
    pub addrs: Vec<(String, Vec<String>)>,
}

/// D2 import: `file` (validated as `staged` by [`crate::config::stage`]) into records. A
/// contact whose endpoint some record anchors attaches to the newest such record: it adds
/// only the grants that record lacks, never writes `blocked`, `removed_ms` or `binding`, and
/// a removed one is skipped. Otherwise a new record is made, with this machine's live
/// binding for the alias (its history keys go on) and the file's `blocked`. An export with
/// the same name and target as a live row reuses its id; otherwise it gets a new one with
/// `home` = this machine.
pub fn import(file: &PeersFile, staged: &Staged, r: &Records) -> Import {
    let mut out = Import::default();
    let now = now_ms();
    // Exports.
    let targets = staged
        .exports
        .values()
        .map(|e| {
            (
                &e.name,
                Some(e.session.to_string()),
                None,
                &e.title,
                &e.description,
            )
        })
        .chain(staged.agents.values().map(|a| {
            (
                &a.name,
                None,
                Some(a.agent.clone()),
                &a.title,
                &a.description,
            )
        }));
    for (name, session, agent, title, description) in targets {
        let same = r.exports.iter().find(|x| {
            x.removed_ms.is_none()
                && x.name.as_deref() == Some(name.as_str())
                && x.session == session
                && x.agent == agent
        });
        let id = match same {
            Some(x) => x.id.clone(),
            None => {
                let id = Uuid::now_v7().to_string();
                let opt = |v: Option<String>| v.map_or(Cell::Null, Cell::Text);
                out.writes.push(RecordWrite::new(
                    RecordTable::Exports,
                    id.clone(),
                    vec![
                        ("name", Cell::Text(name.clone())),
                        ("session", opt(session)),
                        ("agent", opt(agent)),
                        ("title", Cell::Text(title.clone())),
                        ("description", Cell::Text(description.clone())),
                        ("home", Cell::Text(r.me.clone())),
                    ],
                ));
                id
            }
        };
        out.exports.insert(name.clone(), id);
    }
    // Contacts.
    for (alias, facts) in &staged.contacts {
        let Some(entry) = file.peers.get(alias) else {
            continue;
        };
        if !facts.addrs.is_empty() {
            out.addrs.push((
                facts.eid.clone(),
                facts.addrs.iter().map(ToString::to_string).collect(),
            ));
        }
        // Export name → (granted, answer): a granted export, and an answer alone (granted
        // NULL: it abstains on the grant).
        let mut wanted: BTreeMap<&str, (Option<i64>, Option<FirstContact>)> = BTreeMap::new();
        for x in &entry.exports {
            wanted.insert(x, (Some(1), None));
        }
        for (x, a) in &entry.first_contact {
            wanted.entry(x).or_insert((None, None)).1 = Some(*a);
        }
        let existing = record_with_eid(r, &facts.eid, true);
        let (record, fresh) = match existing {
            Some(c) if c.removed_ms.is_some() => {
                out.skipped.push(alias.clone());
                continue;
            }
            Some(c) => {
                // Only a 0→1 change is safe to carry: the file's block applies, its absence
                // never unblocks.
                if facts.blocked && c.blocked.is_none_or(|b| b == 0) {
                    out.writes.push(RecordWrite::new(
                        RecordTable::Contacts,
                        c.id.clone(),
                        vec![("blocked", Cell::Int(1))],
                    ));
                }
                (c.id.clone(), false)
            }
            None => (Uuid::now_v7().to_string(), true),
        };
        if fresh {
            // This machine's live binding for the alias keeps its history, but only when it
            // is still this endpoint's (as `bind_peer_in` minted a new one for a re-pin).
            let pinned = agent::communication::network::contact_endpoint(&facts.eid);
            let binding = r
                .live_bindings
                .get(alias)
                .filter(|(_, endpoint)| *endpoint == pinned)
                .map_or_else(fresh_binding, |(id, _)| format!("peer:{id}"));
            let paired = if facts.paired_ms > 0 {
                facts.paired_ms as i64
            } else {
                now
            };
            out.writes.push(RecordWrite::new(
                RecordTable::Contacts,
                record.clone(),
                vec![
                    ("alias", Cell::Text(alias.clone())),
                    ("name", Cell::Text(facts.display.clone())),
                    ("machine", Cell::Text(facts.machine.clone())),
                    ("binding", Cell::Text(binding)),
                    ("paired_ms", Cell::Int(paired)),
                    ("paired_on", Cell::Text(r.me.clone())),
                    ("blocked", Cell::Int(i64::from(facts.blocked))),
                ],
            ));
            out.writes.push(RecordWrite::new(
                RecordTable::Endpoints,
                format!("{record}|{}", facts.eid),
                vec![
                    ("contact", Cell::Text(record.clone())),
                    ("eid", Cell::Text(facts.eid.clone())),
                    ("paired_ms", Cell::Int(paired)),
                ],
            ));
        }
        for (name, (granted, ans)) in &wanted {
            let Some(xid) = out
                .exports
                .get(*name)
                .or_else(|| staged.export_ids.get(*name))
            else {
                continue;
            };
            let id = format!("{record}|{xid}");
            if !fresh && r.grants.iter().any(|g| g.id == id) {
                continue;
            }
            out.writes.push(RecordWrite::new(
                RecordTable::Grants,
                id,
                vec![
                    ("contact", Cell::Text(record.clone())),
                    ("export", Cell::Text(xid.clone())),
                    ("granted", granted.map_or(Cell::Null, Cell::Int)),
                    (
                        "first_contact",
                        ans.map_or(Cell::Null, |a| Cell::Text(answer_text(a).to_owned())),
                    ),
                ],
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::store::contacts::ExportRow;

    fn eid(n: u8) -> String {
        iroh::SecretKey::from_bytes(&[n; 32]).public().to_string()
    }

    fn contact(id: &str, alias: &str, paired: i64) -> ContactRow {
        ContactRow {
            id: id.into(),
            alias: Some(alias.into()),
            name: Some(alias.into()),
            binding: Some(format!("peer:{}", Uuid::from_u128(paired as u128))),
            paired_ms: Some(paired),
            blocked: Some(0),
            ..ContactRow::default()
        }
    }

    fn anchor(contact: &str, n: u8, paired: i64) -> EndpointRow {
        EndpointRow {
            id: format!("{contact}|{}", eid(n)),
            contact: Some(contact.into()),
            eid: Some(eid(n)),
            paired_ms: Some(paired),
            barred_ms: None,
        }
    }

    fn grant(contact: &str, export: &str, granted: Option<i64>, fc: Option<&str>) -> GrantRow {
        GrantRow {
            id: format!("{contact}|{export}"),
            contact: Some(contact.into()),
            export: Some(export.into()),
            granted,
            first_contact: fc.map(str::to_owned),
        }
    }

    fn chat(id: &str, name: &str, session: runtime::SessionId) -> ExportRow {
        ExportRow {
            id: id.into(),
            name: Some(name.into()),
            session: Some(session.to_string()),
            ..ExportRow::default()
        }
    }

    fn stage(r: &Records) -> Staged {
        stage_records(r, &BTreeSet::new(), tools::Risk::ReadOnly)
    }

    /// Apply `writes` to `r` as the store would (column by column, inserting new rows).
    fn apply(r: &mut Records, writes: &[RecordWrite]) {
        for w in writes {
            let text = |c: &Cell| match c {
                Cell::Text(t) => Some(t.clone()),
                _ => None,
            };
            let int = |c: &Cell| match c {
                Cell::Int(i) => Some(*i),
                _ => None,
            };
            match w.table {
                RecordTable::Contacts => {
                    if !r.contacts.iter().any(|c| c.id == w.id) {
                        r.contacts.push(ContactRow {
                            id: w.id.clone(),
                            ..ContactRow::default()
                        });
                    }
                    let c = r.contacts.iter_mut().find(|c| c.id == w.id).unwrap();
                    for (col, v) in &w.set {
                        match *col {
                            "alias" => c.alias = text(v),
                            "merged_into" => c.merged_into = text(v),
                            "blocked" => c.blocked = int(v),
                            "binding" => c.binding = text(v),
                            "removed_ms" => c.removed_ms = int(v),
                            _ => {}
                        }
                    }
                }
                RecordTable::Exports => {
                    if let Some(x) = r.exports.iter_mut().find(|x| x.id == w.id) {
                        for (col, v) in &w.set {
                            if *col == "name" {
                                x.name = text(v);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 191 (M4): two machines export a chat each under one name: two ids, the newer is
    /// renamed `<name>-2` by the reconcile (the same write on every machine), and a grant
    /// names the id, so it reaches the chat it was given for whatever the names do.
    #[test]
    fn two_exports_of_one_name_stay_two_and_the_newer_is_renamed() {
        let (s1, s2) = (runtime::SessionId::new(), runtime::SessionId::new());
        let x1 = Uuid::now_v7().to_string();
        let x2 = Uuid::now_v7().to_string();
        let mut r = Records {
            contacts: vec![contact("c1", "bob", 1)],
            endpoints: vec![anchor("c1", 1, 1)],
            grants: vec![grant("c1", &x2, Some(1), Some("allow"))],
            exports: vec![chat(&x1, "notes", s1), chat(&x2, "notes", s2)],
            ..Records::default()
        };
        let w = reconcile_writes(&r);
        assert_eq!(
            w,
            vec![RecordWrite::new(
                RecordTable::Exports,
                x2.clone(),
                vec![("name", Cell::Text("notes-2".into()))]
            )]
        );
        apply(&mut r, &w);
        assert!(reconcile_writes(&r).is_empty(), "converged");
        let s = stage(&r);
        assert_eq!(s.exports["notes"].session, s1);
        assert_eq!(s.exports["notes-2"].session, s2);
        assert_eq!(
            s.peers["bob"].exports,
            ["notes-2"],
            "the grant follows its id"
        );
        assert_eq!(s.export_ids["notes-2"], x2);
    }

    /// 191 (D2 merge): two records anchoring one endpoint merge into the older; a grant
    /// held by one duplicate carries over (the other abstains), `granted = 0` and `block` win
    /// where both have a row, a block on either blocks the contact, a chain C→B→A is
    /// flattened to the root, and until the pointers land both are denied.
    #[test]
    fn a_merge_points_at_the_survivor_and_reads_the_strictest_grant() {
        let (a, b, c) = ("a".to_owned(), "b".to_owned(), "c".to_owned());
        let mut r = Records {
            contacts: vec![
                contact(&a, "bob", 1),
                contact(&b, "bob", 2),
                ContactRow {
                    merged_into: Some(b.clone()),
                    ..contact(&c, "bob-old", 3)
                },
            ],
            endpoints: vec![anchor(&a, 1, 1), anchor(&b, 1, 2), anchor(&b, 2, 2)],
            grants: vec![
                grant(&b, "x-only-b", Some(1), Some("allow")),
                grant(&a, "x-both", Some(1), Some("allow")),
                grant(&b, "x-both", Some(0), Some("block")),
                grant(&c, "x-ask", Some(1), None),
                grant(&a, "x-ask", Some(1), Some("allow")),
            ],
            exports: ["x-only-b", "x-both", "x-ask"]
                .iter()
                .map(|x| chat(x, x, runtime::SessionId::new()))
                .collect(),
            ..Records::default()
        };
        let before = stage(&r);
        assert!(
            before.contacts.is_empty(),
            "unmerged duplicates admit nobody"
        );
        assert!(before.deny.contains(&eid(1)) && before.deny.contains(&eid(2)));
        let w = reconcile_writes(&r);
        let pointers: BTreeMap<String, String> = w
            .iter()
            .filter_map(|w| match &w.set[0] {
                ("merged_into", Cell::Text(t)) => Some((w.id.clone(), t.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            pointers,
            BTreeMap::from([(b.clone(), a.clone()), (c.clone(), a.clone())]),
            "both point at the survivor (the chain flattened)"
        );
        apply(&mut r, &w);
        assert!(reconcile_writes(&r).is_empty(), "converged");
        let s = stage(&r);
        let f = &s.contacts["bob"];
        assert_eq!(f.id, a);
        assert_eq!(f.eids.len(), 2, "the union of the anchors");
        assert_eq!(s.peers["bob"].exports, ["x-ask", "x-only-b"]);
        assert_eq!(
            f.first_contact["x-only-b"],
            FirstContact::Allow,
            "carried over"
        );
        assert_eq!(f.first_contact["x-both"], FirstContact::Block, "block wins");
        assert_eq!(
            f.first_contact["x-ask"],
            FirstContact::Ask,
            "ask beats allow"
        );
        // A block on a merged-away record blocks the contact.
        r.contacts[1].blocked = Some(1);
        let s = stage(&r);
        assert!(s.contacts["bob"].blocked && !s.peers.contains_key("bob"));
        assert!(s.deny.contains(&eid(1)) && s.deny.contains(&eid(2)));
    }

    /// 191 (D2): a merge chain that loops, or runs past [`CHAIN_MAX`], quarantines its group
    /// with its endpoints denied.
    #[test]
    fn a_looping_or_endless_chain_quarantines_the_group() {
        let mut looped = Records {
            contacts: vec![
                ContactRow {
                    merged_into: Some("b".into()),
                    ..contact("a", "bob", 1)
                },
                ContactRow {
                    merged_into: Some("a".into()),
                    ..contact("b", "bob", 2)
                },
            ],
            endpoints: vec![anchor("a", 1, 1)],
            ..Records::default()
        };
        let s = stage(&looped);
        assert!(s.contacts.is_empty());
        assert!(s.deny.contains(&eid(1)));
        assert!(s.quarantined.iter().all(|q| q.problem.contains("loops")));
        assert!(
            reconcile_writes(&looped).is_empty(),
            "no pointer is written into a loop"
        );
        // A chain of 18 records: the root is out of reach.
        looped.contacts = (0..18)
            .map(|i| ContactRow {
                merged_into: (i > 0).then(|| format!("r{}", i - 1)),
                ..contact(&format!("r{i}"), "bob", i64::from(i))
            })
            .collect();
        looped.endpoints = vec![anchor("r17", 1, 1)];
        let s = stage(&looped);
        assert!(s.deny.contains(&eid(1)));
        assert!(s.contacts.is_empty(), "the whole group, its root included");
        assert_eq!(s.quarantined.len(), 18);
    }

    /// 191 (D2): a malformed record is quarantined alone and its endpoints denied; a blocked
    /// record beside it still applies, and a sound one still stages.
    #[test]
    fn a_bad_row_denies_its_endpoints_and_a_block_beside_it_applies() {
        let r = Records {
            contacts: vec![
                contact("bad", "Not An Alias!", 1),
                ContactRow {
                    blocked: Some(1),
                    ..contact("blk", "carol", 2)
                },
                contact("ok", "dave", 3),
                ContactRow {
                    binding: Some("peer:00000000-0000-0000-0000-00000000000a".into()),
                    ..contact("rev", "erin", 4)
                },
            ],
            endpoints: vec![
                anchor("bad", 1, 1),
                anchor("blk", 2, 2),
                anchor("ok", 3, 3),
                anchor("rev", 4, 4),
            ],
            revoked: BTreeSet::from(["00000000-0000-0000-0000-00000000000a".to_owned()]),
            ..Records::default()
        };
        let s = stage(&r);
        let problems: BTreeMap<&str, &str> = s
            .quarantined
            .iter()
            .map(|q| (q.id.as_str(), q.problem.as_str()))
            .collect();
        assert_eq!(problems.len(), 2);
        assert!(problems["bad"].contains("malformed"));
        assert!(
            problems["rev"].contains("revoked"),
            "a replica never un-revokes"
        );
        assert!(s.deny.contains(&eid(1)) && s.deny.contains(&eid(4)));
        assert!(s.contacts["carol"].blocked && s.deny.contains(&eid(2)));
        assert!(s.peers.contains_key("dave") && !s.deny.contains(&eid(3)));
        // One of our own machines is never a contact.
        let s = stage_records(&r, &BTreeSet::from([eid(3)]), tools::Risk::ReadOnly);
        assert!(!s.contacts.contains_key("dave") && s.deny.contains(&eid(3)));
    }

    /// 191 (D2): a removed record's endpoint stays refused (a stale copy never admits it)
    /// until a newer ceremony pins it in a live record.
    #[test]
    fn a_removed_endpoint_is_refused_until_a_newer_ceremony() {
        let mut r = Records {
            contacts: vec![ContactRow {
                removed_ms: Some(10),
                ..contact("old", "bob", 1)
            }],
            endpoints: vec![anchor("old", 1, 1)],
            ..Records::default()
        };
        assert!(stage(&r).removed.contains(&eid(1)));
        r.contacts.push(contact("new", "bob", 20));
        r.endpoints.push(anchor("new", 1, 20));
        let s = stage(&r);
        assert!(!s.removed.contains(&eid(1)) && s.peers.contains_key("bob"));
    }

    /// 191 (D2): two contacts named alike converge: the older keeps the alias and the newer
    /// is renamed, by the same write on every machine.
    #[test]
    fn an_alias_collision_converges() {
        let mut r = Records {
            contacts: vec![
                contact("a", "bob", 1),
                contact("b", "bob", 2),
                contact("c", "bob-2", 3),
            ],
            endpoints: vec![anchor("a", 1, 1), anchor("b", 2, 2), anchor("c", 3, 3)],
            ..Records::default()
        };
        assert!(
            stage(&r).quarantined.iter().any(|q| q.id == "b"),
            "the newer waits for its rename"
        );
        let w = reconcile_writes(&r);
        assert_eq!(
            w,
            vec![RecordWrite::new(
                RecordTable::Contacts,
                "b",
                vec![("alias", Cell::Text("bob-3".into()))]
            )]
        );
        apply(&mut r, &w);
        assert!(reconcile_writes(&r).is_empty());
        let s = stage(&r);
        assert_eq!(
            s.contacts.keys().collect::<Vec<_>>(),
            ["bob", "bob-2", "bob-3"]
        );
    }

    /// 191 (D2 import, S2): an entry whose endpoint a removed record anchors is skipped (never
    /// revived); one a live record anchors adds only the grants that record lacks, never
    /// writes `removed_ms` or `binding`, and carries a block (0→1) but never an unblock; a
    /// new one keeps this machine's binding only while it is still that endpoint's.
    #[test]
    fn the_import_never_revives_and_never_writes_blocked_on_reuse() {
        let file: PeersFile = serde_yaml::from_str(&format!(
            "peers:\n  gone:\n    endpoint: {}\n  bob:\n    endpoint: {}\n    blocked: true\n    exports: [proj, ops]\n  new:\n    endpoint: {}\n    exports: [proj]\n    first_contact: {{ ops: block }}\n  carol:\n    endpoint: {}\n  moved:\n    endpoint: {}\nexports:\n  proj: {{ session: \"00000000-0000-0000-0000-000000000003\", name: Proj }}\n  ops: {{ agent: theuth, name: Ops }}\n",
            eid(1),
            eid(2),
            eid(3),
            eid(4),
            eid(5)
        ))
        .unwrap();
        let staged = crate::config::stage(&file, &|_| None, None, tools::Risk::ReadOnly).unwrap();
        let proj = chat(
            "xp",
            "proj",
            "00000000-0000-0000-0000-000000000003".parse().unwrap(),
        );
        let iroh = |n: u8| agent::communication::network::contact_endpoint(&eid(n));
        let r = Records {
            me: "me".into(),
            contacts: vec![
                ContactRow {
                    removed_ms: Some(5),
                    ..contact("gone-rec", "gone", 1)
                },
                contact("bob-rec", "bob", 2),
                ContactRow {
                    blocked: Some(1),
                    ..contact("carol-rec", "carol", 3)
                },
            ],
            endpoints: vec![
                anchor("gone-rec", 1, 1),
                anchor("bob-rec", 2, 2),
                anchor("carol-rec", 4, 3),
            ],
            grants: vec![grant("bob-rec", "xp", Some(0), None)],
            exports: vec![proj],
            live_bindings: BTreeMap::from([
                (
                    "new".to_owned(),
                    ("00000000-0000-0000-0000-0000000000aa".to_owned(), iroh(3)),
                ),
                (
                    "moved".to_owned(),
                    ("00000000-0000-0000-0000-0000000000bb".to_owned(), iroh(6)),
                ),
            ]),
            ..Records::default()
        };
        let imp = import(&file, &staged, &r);
        assert_eq!(imp.skipped, ["gone"]);
        assert_eq!(
            imp.exports["proj"], "xp",
            "same name and target: the id is reused"
        );
        let ops = imp.exports["ops"].clone();
        assert_ne!(ops, "xp");
        assert!(
            !imp.writes
                .iter()
                .any(|w| w.id == "gone-rec" || w.id.starts_with("gone-rec|")),
            "nothing written onto the removed record"
        );
        let bob: Vec<&RecordWrite> = imp
            .writes
            .iter()
            .filter(|w| w.id == "bob-rec" || w.id.starts_with("bob-rec|"))
            .collect();
        assert_eq!(
            bob.len(),
            2,
            "the block and the grant bob's record lacks: {bob:?}"
        );
        assert_eq!(
            *bob[0],
            RecordWrite::new(
                RecordTable::Contacts,
                "bob-rec",
                vec![("blocked", Cell::Int(1))]
            ),
            "the file's block is carried"
        );
        assert_eq!(bob[1].id, format!("bob-rec|{ops}"));
        assert!(
            !imp.writes.iter().any(|w| w.id == "carol-rec"),
            "an unblocked entry never unblocks a blocked record"
        );
        let moved = imp
            .writes
            .iter()
            .find(|w| {
                w.table == RecordTable::Contacts
                    && w.set.contains(&("alias", Cell::Text("moved".into())))
            })
            .expect("a record for moved");
        assert!(
            !moved.set.contains(&(
                "binding",
                Cell::Text("peer:00000000-0000-0000-0000-0000000000bb".into())
            )),
            "another endpoint's binding is not inherited"
        );
        let new = imp
            .writes
            .iter()
            .find(|w| {
                w.table == RecordTable::Contacts
                    && w.set.contains(&("alias", Cell::Text("new".into())))
            })
            .expect("a record for new");
        assert!(new.set.contains(&(
            "binding",
            Cell::Text("peer:00000000-0000-0000-0000-0000000000aa".into())
        )));
        assert!(new.set.contains(&("paired_on", Cell::Text("me".into()))));
        let answer = imp
            .writes
            .iter()
            .find(|w| w.id == format!("{}|{ops}", new.id))
            .expect("the answer-only row");
        assert!(
            answer.set.contains(&("granted", Cell::Null)),
            "it abstains on the grant"
        );
        assert!(
            answer
                .set
                .contains(&("first_contact", Cell::Text("block".into())))
        );
    }

    /// 191 (D2): a live duplicate of a removed record (made before the removal synced, or a
    /// re-pair under clock skew) whose anchors were all removed is never admitted: no route,
    /// listed with its problem, its endpoint free to pair again; a newer ceremony admits it. A
    /// member removed through its root takes the root's removal time, so a re-pair works.
    #[test]
    fn a_live_duplicate_of_a_removed_record_is_not_admitted() {
        let mut r = Records {
            contacts: vec![
                ContactRow {
                    removed_ms: Some(10),
                    ..contact("old", "bob", 1)
                },
                contact("dup", "bob", 5),
            ],
            endpoints: vec![anchor("old", 1, 1), anchor("dup", 1, 5)],
            ..Records::default()
        };
        let s = stage(&r);
        assert!(!s.peers.contains_key("bob") && !s.contacts.contains_key("bob"));
        assert!(
            s.quarantined
                .iter()
                .any(|q| q.id == "dup" && q.problem.contains("removed"))
        );
        assert!(s.removed.contains(&eid(1)) && !s.deny.contains(&eid(1)));
        assert!(
            s.quarantined.iter().all(|q| q.hold.is_none()),
            "its binding goes"
        );
        r.endpoints[1].paired_ms = Some(20);
        assert!(
            stage(&r).peers.contains_key("bob"),
            "a newer ceremony admits it"
        );

        let r = Records {
            contacts: vec![
                ContactRow {
                    removed_ms: Some(10),
                    ..contact("root", "carol", 1)
                },
                ContactRow {
                    merged_into: Some("root".into()),
                    ..contact("member", "carol", 2)
                },
                contact("again", "carol", 20),
            ],
            endpoints: vec![anchor("member", 2, 2), anchor("again", 2, 20)],
            ..Records::default()
        };
        let s = stage(&r);
        assert!(s.peers.contains_key("carol") && !s.removed.contains(&eid(2)));
    }

    /// 191 (D2): a quarantine of lag or of a convergence in flight keeps the binding (never
    /// revoked for good); a grant naming an export this machine lacks is skipped, the contact
    /// stays.
    #[test]
    fn lag_keeps_the_binding_and_a_missing_export_skips_only_its_grant() {
        let r = Records {
            contacts: vec![
                contact("a", "bob", 1),
                contact("b", "bob", 2),
                ContactRow {
                    merged_into: Some("not-here".into()),
                    ..contact("c", "carol", 3)
                },
                contact("d", "dave", 4),
            ],
            endpoints: vec![
                anchor("a", 1, 1),
                anchor("b", 2, 2),
                anchor("c", 3, 3),
                anchor("d", 4, 4),
            ],
            grants: vec![grant("d", "x-missing", Some(1), None)],
            ..Records::default()
        };
        let s = stage(&r);
        let held: BTreeMap<&str, bool> = s
            .quarantined
            .iter()
            .map(|q| (q.id.as_str(), q.hold.is_some()))
            .collect();
        assert_eq!(
            held,
            BTreeMap::from([("b", true), ("c", true)]),
            "renaming and lag"
        );
        assert!(
            s.peers["dave"].exports.is_empty(),
            "the unresolvable grant is skipped"
        );
    }
}
