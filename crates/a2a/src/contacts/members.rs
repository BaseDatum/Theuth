//! 191 (D1, P2): the rosters' traffic. Our side signs, co-signs and prunes each live record's
//! chain in the reconcile ([`Ctx::sign_rosters`]), carries the last versions in the card
//! a contact fetches ([`Ctx::carried_for`]), pushes them on `NOTICE_PATH` after a signing
//! ([`Ctx::push_rosters`]) and serves proof paths ([`Ctx::roster_path`]). The contact's side
//! keeps its own verified state per record: it moves on our signed copies only
//! ([`Ctx::ingest`]), bootstraps from the record's pin and watches the other machines' heads
//! ([`Ctx::roster_round`]). Every path that cannot prove its trust denies, or only alerts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent::store::contacts::{RecordWrite, Records};

use super::records::{self, Admit};
use super::rosters::{self, Carried, Chain, Copy, PathAnswer, PeerHead, Seen, Verified};
use crate::Ctx;

/// A failed push waits this long first, doubling up to [`PUSH_MAX`].
pub const PUSH_FIRST: Duration = Duration::from_secs(2);
/// The longest wait between two tries of one push.
pub const PUSH_MAX: Duration = Duration::from_secs(600);
/// Two inflation observations count as two refreshes only this far apart.
pub const INFLATION_GAP: Duration = Duration::from_secs(5);
/// Machines of ours a round dials at most per record.
const TARGETS_MAX: usize = 8;
/// Continuations followed per proof path.
const CONTINUATIONS_MAX: usize = 16;

/// The contact side's per-process roster bookkeeping.
#[derive(Default)]
pub(crate) struct Mem {
    /// Refresh rounds run.
    pub(crate) round: u64,
    /// When the last round started.
    pub(crate) last_round: Option<Instant>,
    /// Record → the round of its last equal-version flip (at most one per round).
    flipped: HashMap<String, u64>,
    /// (record, peer machine) → rounds this machine has been merely behind that peer.
    behind: HashMap<(String, String), u32>,
    /// Record → (last counted observation, count) of an inflated proof-path request.
    pub(crate) seen: HashMap<String, (Instant, u32)>,
    /// Record → (contact alias, alert text): the raised inflation alerts (alert only).
    pub(crate) alerts: BTreeMap<String, (String, String, u64)>,
}

/// This mesh's live machines as the reconcile sees them (this one included).
pub(crate) fn family(net: Option<&mesh::net::Net>) -> BTreeSet<String> {
    net.map(|n| {
        n.paired_ids()
            .iter()
            .map(ToString::to_string)
            .chain([n.id.0.clone()])
            .collect()
    })
    .unwrap_or_default()
}

/// Query parameter `k` of `q`.
fn param<'a>(q: &'a str, k: &str) -> Option<&'a str> {
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(key, _)| *key == k)
        .map(|(_, v)| v)
}

/// A query value that is a uuid, an endpoint id or a hash: hex digits and dashes only.
fn plain(v: &str) -> bool {
    !v.is_empty() && v.len() <= 80 && v.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

impl Ctx {
    // -----------------------------------------------------------------------
    // Our side.
    // -----------------------------------------------------------------------

    /// Our mesh's members with their `added_ms` (the cap's and the signature order's
    /// "oldest first").
    async fn mesh_members(&self) -> Result<Vec<(String, u64)>, String> {
        let net = self.net().ok_or("this engine has no mesh endpoint")?;
        net.members()
            .await
            .map_err(|e| format!("mesh members: {e:#}"))
    }

    /// 191 (D1): for each live record, sign its chain at latest + 1 when the membership this
    /// machine computes differs from the latest content, co-sign the latest when it equals it
    /// and this machine has not signed it, else nothing; then prune to the hop chains plus
    /// the last [`rosters::CARRIED`] versions (at most [`rosters::SIGS_MAX`] copies a content).
    /// Nothing is signed for a removed record, and its rows go. Only after `contacts_ready`.
    /// Returns whether this machine signed.
    pub(crate) async fn sign_rosters(&self, r: &Records) -> Result<bool, String> {
        let Some(net) = self.net() else {
            return Ok(false);
        };
        let members = self.mesh_members().await?;
        let added: BTreeMap<String, u64> = members.iter().cloned().collect();
        let me = net.id.0.clone();
        let a = records::analyze(r);
        let mut w: Vec<RecordWrite> = Vec::new();
        let mut keep_ids: BTreeSet<String> = BTreeSet::new();
        let mut signed = false;
        for c in a.groups.iter().flat_map(|g| g.members.iter()) {
            let pins = records::our_pins(r, c);
            let mine = rosters::membership(&members, &pins);
            let mut copies = records::chain_copies(r, &c.id);
            if let Some(v) = rosters::decide(&copies, &mine, &me, &pins, &added) {
                let copy = rosters::sign(&c.id, v, &mine, &me, &|m| net.sign(m));
                tracing::info!(record = %c.id, version = v, members = mine.len(), "191: roster signed");
                w.push(records::roster_row(&copy));
                copies.push(copy);
                signed = true;
            }
            // The hop chains are recomputed with the new copy before anything is dropped.
            let keep = rosters::keep(&copies, &pins);
            let over: BTreeSet<String> = rosters::over_cap(&copies, &pins, &added)
                .into_iter()
                .map(records::roster_id)
                .collect();
            keep_ids.extend(
                copies
                    .iter()
                    .filter(|x| keep.contains(&x.version))
                    .map(records::roster_id)
                    .filter(|id| !over.contains(id)),
            );
        }
        // A row of a live chain that is not kept (pruned, over the cap, malformed or not
        // verifying) goes, and so does every row of a removed record. Rows of a record this
        // machine lacks or cannot place (lag) stay.
        let live: BTreeSet<&str> = a
            .groups
            .iter()
            .flat_map(|g| g.members.iter().map(|m| m.id.as_str()))
            .collect();
        let removed: BTreeSet<&str> = a.removed.iter().map(|(c, _)| c.id.as_str()).collect();
        for row in &r.rosters.rows {
            let of = row.contact.as_deref().unwrap_or_default();
            if removed.contains(of) || (live.contains(of) && !keep_ids.contains(&row.id)) {
                w.push(RecordWrite::delete(
                    agent::store::contacts::RecordTable::Rosters,
                    row.id.clone(),
                ));
            }
        }
        if !w.is_empty() {
            let agents = self.agents()?;
            self.ask(
                &agents,
                agent::communication::network::WriteContactRecords(w),
            )
            .await?;
        }
        Ok(signed)
    }

    /// The members/v1 payload for the contact whose group `root` heads: the last
    /// [`rosters::CARRIED`] versions of each of its live records' chains. Empty while no
    /// chain exists (the extension is then omitted). A removed record's chain is never in it.
    pub(crate) fn carried_for(r: &Records, root: &str, added: &BTreeMap<String, u64>) -> Carried {
        let a = records::analyze(r);
        let Some(g) = records::group_of(&a, root) else {
            return Carried::default();
        };
        Carried {
            chains: g
                .members
                .iter()
                .filter_map(|c| {
                    let copies = records::chain_copies(r, &c.id);
                    (!copies.is_empty())
                        .then(|| rosters::carried(&c.id, &copies, &records::our_pins(r, c), added))
                })
                .collect(),
        }
    }

    /// The payload carried to the caller `eid` (its own group's chains).
    pub(crate) async fn carried_to(&self, eid: &str) -> Result<Carried, String> {
        let root = {
            let live = self.shared.read();
            let alias = live.staged.contact_by_eid(eid).ok_or("not a contact")?;
            live.staged.contacts[alias].id.clone()
        };
        let r = self.records().await?;
        let added: BTreeMap<String, u64> = self.mesh_members().await?.into_iter().collect();
        Ok(Self::carried_for(&r, &root, &added))
    }

    /// 191 (D1 push): our chains to every machine of every live contact (`all`, after a
    /// signing), else only to a contact not pushed to since (newly admitted, or at boot), as
    /// a `NOTICE_PATH` body. A failed post is tried again from [`PUSH_FIRST`], doubling up to
    /// [`PUSH_MAX`]; a newer push to the same endpoint supersedes it.
    pub(crate) async fn push_rosters(&self, r: &Records, all: bool) {
        let live = self.shared.read().clone();
        let due: Vec<&crate::config::ContactFacts> = {
            let mut s = self.state.lock();
            if all {
                s.pushed_to.clear();
            }
            live.staged
                .contacts
                .values()
                .filter(|f| !f.blocked && s.pushed_to.insert(f.id.clone()))
                .collect()
        };
        if due.is_empty() {
            return;
        }
        let (Some(net), Ok(members)) = (self.net(), self.mesh_members().await) else {
            return;
        };
        let added: BTreeMap<String, u64> = members.into_iter().collect();
        for f in due {
            let body = Self::carried_for(r, &f.id, &added);
            if body.chains.is_empty() {
                continue;
            }
            let Ok(body) = serde_json::to_vec(&body) else {
                continue;
            };
            let body = bytes::Bytes::from(body);
            for eid in &f.eids {
                let generation = {
                    let mut p = self.pushes.lock();
                    let g = p.entry(eid.clone()).or_default();
                    *g += 1;
                    *g
                };
                let (net, pushes, eid, body) =
                    (net.clone(), self.pushes.clone(), eid.clone(), body.clone());
                let stop = self.rt.shutdown.clone();
                tokio::spawn(async move {
                    let mut wait = PUSH_FIRST;
                    loop {
                        if pushes.lock().get(&eid) != Some(&generation) {
                            return;
                        }
                        match super::offers::call_with(
                            &net,
                            &eid,
                            hyper::Method::POST,
                            super::offers::NOTICE_PATH,
                            Some(body.clone()),
                        )
                        .await
                        {
                            Ok((s, _)) if s.is_success() => return,
                            Ok((s, _)) if s == hyper::StatusCode::PAYLOAD_TOO_LARGE => {
                                tracing::warn!(%eid, "191: roster push refused as too large");
                                return;
                            }
                            Ok((s, _)) => {
                                tracing::debug!(%eid, "191: roster push answered {s}; retrying")
                            }
                            Err(e) => tracing::debug!(%eid, "191: roster push: {e}; retrying"),
                        }
                        tokio::select! {
                            () = stop.cancelled() => return,
                            () = tokio::time::sleep(wait) => {}
                        }
                        wait = (wait * 2).min(PUSH_MAX);
                    }
                });
            }
        }
    }

    /// `GET ROSTER_PATH?chain=<id>&pin=<eid>&from=<v>[&to=<v>][&head=<hash>]` from the
    /// admitted contact `remote`: the proof path of one of the caller's own records, from
    /// `pin` (one of the machines of ours that ran a ceremony into it), in greedy hops. The
    /// request is also where the inflation alert looks (alert only).
    pub(crate) async fn roster_path(
        &self,
        remote: &str,
        query: &str,
    ) -> Result<PathAnswer, (hyper::StatusCode, String)> {
        use hyper::StatusCode as S;
        let bad = |m: &str| (S::BAD_REQUEST, m.to_owned());
        let chain = param(query, "chain")
            .filter(|v| plain(v))
            .ok_or_else(|| bad("chain"))?;
        let pin = param(query, "pin")
            .filter(|v| plain(v))
            .ok_or_else(|| bad("pin"))?;
        let num = |k: &str| -> Result<Option<u64>, (S, String)> {
            param(query, k)
                .map(|v| v.parse::<u64>().map_err(|_| bad(k)))
                .transpose()
        };
        let from = num("from")?.unwrap_or(0);
        let to = num("to")?;
        let head = param(query, "head").filter(|v| plain(v));
        let (root, alias) = {
            let live = self.shared.read();
            let alias = live
                .staged
                .contact_by_eid(remote)
                .ok_or((S::UNAUTHORIZED, "not a contact".to_owned()))?;
            (live.staged.contacts[alias].id.clone(), alias.to_owned())
        };
        let r = self
            .records()
            .await
            .map_err(|e| (S::INTERNAL_SERVER_ERROR, e))?;
        let a = records::analyze(&r);
        let c = records::group_of(&a, &root)
            .and_then(|g| g.members.iter().find(|m| m.id == chain))
            .ok_or((S::NOT_FOUND, "not one of your records".to_owned()))?;
        if !records::our_pins(&r, c).contains(pin) {
            return Err((S::FORBIDDEN, "not a pin of that record".to_owned()));
        }
        let copies = records::chain_copies(&r, chain);
        if self.ready.load(std::sync::atomic::Ordering::SeqCst) {
            let latest = rosters::latest(&copies).map_or(0, |x| x.version);
            let never = match (to, head) {
                (Some(v), Some(h)) => {
                    copies.iter().any(|x| x.version == v)
                        && !copies.iter().any(|x| x.version == v && x.hash() == h)
                }
                _ => false,
            };
            if to.is_some_and(|v| v > latest) || never {
                self.observe(&root, &alias);
            }
        }
        let hops = rosters::hops(&copies, pin, to.unwrap_or(u64::MAX));
        Ok(rosters::path_answer(chain, &hops, from))
    }

    /// One inflated proof-path request for `record`: on the second at least
    /// [`INFLATION_GAP`] after the first, the alert is raised (`Contact.problem` and an
    /// Attention item). It never denies: a partition must not lock out an honest contact.
    pub(crate) fn observe(&self, record: &str, alias: &str) {
        let raised = {
            let mut m = self.rosters.lock();
            let now = Instant::now();
            let e = m.seen.entry(record.to_owned()).or_insert((now, 0));
            if e.1 == 0 || now.duration_since(e.0) >= INFLATION_GAP {
                *e = (now, e.1 + 1);
            }
            if e.1 >= 2 && !m.alerts.contains_key(record) {
                m.alerts.insert(
                    record.to_owned(),
                    (
                        alias.to_owned(),
                        rosters::inflated(alias),
                        mesh::identity::now_ms(),
                    ),
                );
                true
            } else {
                false
            }
        };
        if raised {
            tracing::warn!(contact = %alias, %record, "191: {}", rosters::inflated(alias));
            super::nudge_attention();
        }
    }

    // -----------------------------------------------------------------------
    // The contact's side.
    // -----------------------------------------------------------------------

    /// Copies served by `from` (its card's members/v1, or a push): every record of the group
    /// `from` is admitted for moves on by the accept rule, or takes its base (D1). A record
    /// based on another machine of this mesh but not here adopts only a corroborated head.
    /// Nothing another machine of this mesh holds is ever adopted. Returns whether a state
    /// changed (then the gate is restaged).
    pub(crate) async fn ingest(&self, carried: &Carried, from: &str) -> bool {
        let copies = rosters::verified(carried.copies());
        if copies.is_empty() {
            return false;
        }
        let root = {
            let live = self.shared.read();
            match live.staged.contact_by_eid(from) {
                Some(alias) => live.staged.contacts[alias].id.clone(),
                None => return false,
            }
        };
        let changed = {
            let _serial = self.roster_lock.lock().await;
            let Ok(r) = self.records().await else {
                return false;
            };
            let a = records::analyze(&r);
            let Some(g) = records::group_of(&a, &root) else {
                return false;
            };
            let me = r.me.clone();
            let family = family(self.net().as_deref());
            let now = mesh::identity::now_ms() as i64;
            let mut w = Vec::new();
            for m in &g.members {
                let pins = records::pins_of(&r, &m.id);
                let conflict = records::heads_of(&r, &m.id, &me, &family, true)
                    .iter()
                    .any(|h| h.machine == me && h.conflict);
                let next = match records::admit_of(&r, &m.id, &family, now) {
                    Admit::Roster(mut l) => {
                        let before = l.clone();
                        let mut flips = self.flip_budget(&m.id);
                        let moved = rosters::advance(&mut l, &copies, &mut flips, None);
                        self.spend_flip(&m.id, flips);
                        if moved {
                            let h = l.head();
                            tracing::info!(record = %m.id, version = h.version, members = h.members.len(), "191: contact roster accepted");
                        }
                        (l != before).then_some((l, moved))
                    }
                    Admit::Pins => rosters::base(&pins, &copies).map(|mut l| {
                        rosters::advance(&mut l, &copies, &mut 0, None);
                        tracing::info!(record = %m.id, version = l.head().version, "191: contact roster based on its pin's copy");
                        (l, true)
                    }),
                    Admit::Unverified => {
                        let peers = records::heads_of(&r, &m.id, &me, &family, false);
                        rosters::base(&pins, &copies).and_then(|mut l| {
                            rosters::advance(&mut l, &copies, &mut 0, None);
                            let h = l.head();
                            let ok = rosters::adopt((h.version, h.hash.as_str()), &peers, 1);
                            ok.then_some((l, true))
                        })
                    }
                    Admit::Disagree => None,
                };
                if let Some((l, moved)) = next {
                    w.push(records::local_row(&m.id, &l));
                    if moved {
                        w.push(records::head_row(&m.id, &me, &l, conflict));
                    }
                }
            }
            !w.is_empty() && self.write_rows(w).await
        };
        if changed {
            let _ = self.reconcile_now().await;
        }
        changed
    }

    /// At most one equal-version flip per record per round.
    fn flip_budget(&self, record: &str) -> usize {
        let m = self.rosters.lock();
        usize::from(m.flipped.get(record) != Some(&m.round))
    }

    fn spend_flip(&self, record: &str, left: usize) {
        if left == 0 {
            let mut m = self.rosters.lock();
            let round = m.round;
            m.flipped.insert(record.to_owned(), round);
        }
    }

    async fn write_rows(&self, w: Vec<RecordWrite>) -> bool {
        let Ok(agents) = self.agents() else {
            return false;
        };
        match self
            .ask(
                &agents,
                agent::communication::network::WriteContactRecords(w),
            )
            .await
        {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("191: roster state not written: {e}");
                false
            }
        }
    }

    /// One proof path of `chain` from `target`, from `pin` (following `next`), its copies
    /// verified one by one (the path itself is verified by the caller, from the pins).
    async fn fetch_path(
        &self,
        target: &str,
        chain: &str,
        pin: &str,
        to: Option<(u64, &str)>,
    ) -> Result<Vec<Copy>, String> {
        let net = self.net().ok_or("this engine has no mesh endpoint")?;
        let mut from = 0;
        let mut out = Vec::new();
        for _ in 0..CONTINUATIONS_MAX {
            let mut q = format!(
                "{}?chain={chain}&pin={pin}&from={from}",
                rosters::ROSTER_PATH
            );
            if let Some((v, h)) = to {
                q.push_str(&format!("&to={v}&head={h}"));
            }
            let (s, body) =
                super::offers::call_with(&net, target, hyper::Method::GET, &q, None).await?;
            if !s.is_success() {
                return Err(format!("proof path: HTTP {s}"));
            }
            let a: PathAnswer =
                serde_json::from_slice(&body).map_err(|e| format!("proof path: {e}"))?;
            if a.chain != chain {
                return Err("proof path of another chain".into());
            }
            out.extend(
                Chain {
                    chain: a.chain,
                    versions: a.versions,
                }
                .copies(),
            );
            match a.next {
                Some(n) if n > from => from = n,
                _ => break,
            }
        }
        Ok(rosters::verified(out))
    }

    /// Proof paths of `chain` from up to [`TARGETS_MAX`] of `targets` (asking until
    /// [`rosters::SOURCES_MIN`] verified), each verified by this machine from `pins` alone.
    async fn paths(
        &self,
        targets: &[String],
        chain: &str,
        pins: &BTreeSet<String>,
        to: Option<(u64, &str)>,
    ) -> Vec<(Verified, Vec<Copy>)> {
        let mut out = Vec::new();
        for t in targets.iter().take(TARGETS_MAX) {
            for pin in pins {
                match self.fetch_path(t, chain, pin, to).await {
                    Ok(copies) => {
                        if let Some(v) = rosters::verify_path(pins, chain, &copies) {
                            out.push((v, copies));
                            break;
                        }
                        tracing::info!(target = %t, %chain, "191: proof path does not verify from the pin");
                    }
                    Err(e) => tracing::debug!(target = %t, %chain, "191: proof path: {e}"),
                }
            }
            if out.len() >= rosters::SOURCES_MIN {
                break;
            }
        }
        out
    }

    /// 191 (D1): one refresh round of the contact side, at most every [`rosters::ROUND`]:
    /// the cards of every trusted member are fetched again (when older than the TTL); a
    /// record based elsewhere but not here bootstraps from its pin (a corroborated head only);
    /// a record with state compares it with every live machine's head (merely behind for
    /// [`rosters::STALE_REFRESHES`] rounds, or diverging: one proof path, then a conflict only
    /// when it reached the other head and differs there), and publishes its own head.
    pub(crate) async fn roster_round(self: &Arc<Self>) {
        if !self.ready.load(std::sync::atomic::Ordering::SeqCst) || self.net().is_none() {
            return;
        }
        {
            let mut m = self.rosters.lock();
            m.round += 1;
            m.last_round = Some(Instant::now());
        }
        self.refresh_stale_offers().await;
        let changed = {
            let _serial = self.roster_lock.lock().await;
            let Ok(r) = self.records().await else { return };
            let a = records::analyze(&r);
            let me = r.me.clone();
            let family = family(self.net().as_deref());
            let now = mesh::identity::now_ms() as i64;
            let mut w = Vec::new();
            // Dial and chain hints are untrusted: any machine's row gives them, a retired
            // one's included; only live machines' heads count for adoption and divergence.
            let anyone: BTreeSet<String> =
                r.heads.iter().filter_map(|h| h.machine.clone()).collect();
            for m in a.groups.iter().flat_map(|g| g.members.iter()) {
                let pins = records::pins_of(&r, &m.id);
                let peers = records::heads_of(&r, &m.id, &me, &family, false);
                let mine = records::heads_of(&r, &m.id, &me, &family, true)
                    .into_iter()
                    .find(|h| h.machine == me);
                let hinted = records::heads_of(&r, &m.id, &me, &anyone, false);
                let hints: Vec<String> = hinted.iter().flat_map(|p| p.members.clone()).collect();
                match records::admit_of(&r, &m.id, &family, now) {
                    Admit::Pins => {}
                    Admit::Unverified => {
                        w.extend(
                            self.bootstrap(&m.id, &me, &pins, &peers, &hinted, &family)
                                .await,
                        );
                    }
                    Admit::Roster(_) | Admit::Disagree => {
                        // Under a conflict too: it is recomputed, and cleared once heads agree.
                        let Some(l) = records::local_of(&r, &m.id) else {
                            continue;
                        };
                        w.extend(
                            self.diverge(
                                &m.id,
                                &me,
                                l,
                                &pins,
                                &peers,
                                &hints,
                                mine.as_ref(),
                                &family,
                            )
                            .await,
                        );
                    }
                }
            }
            !w.is_empty() && self.write_rows(w).await
        };
        if changed {
            let _ = self.reconcile_now().await;
        }
    }

    /// A record this machine has no state for, based elsewhere: proof paths from its pins,
    /// of the chains and dialled by the machines the heads rows name (`hinted`: untrusted, a
    /// retired machine's row included); the highest verified head, adopted only if
    /// corroborated by a live machine's head (`peers`) or two sources ([`rosters::adopt`]).
    /// Otherwise nothing: the record stays denied.
    async fn bootstrap(
        &self,
        record: &str,
        me: &str,
        pins: &BTreeSet<String>,
        peers: &[PeerHead],
        hinted: &[PeerHead],
        family: &BTreeSet<String>,
    ) -> Vec<RecordWrite> {
        let mut chains: Vec<String> = hinted.iter().map(|p| p.chain.clone()).collect();
        chains.retain(|c| plain(c));
        chains.sort();
        chains.dedup();
        let hints = hinted.iter().flat_map(|p| p.members.iter());
        let targets = dial_order(pins.iter().chain(hints), family);
        let mut best: Option<(Verified, usize)> = None;
        for chain in &chains {
            let got = self.paths(&targets, chain, pins, None).await;
            for (v, _) in &got {
                let h = v.local.head();
                let agreed = got
                    .iter()
                    .filter(|(x, _)| x.local.head().hash == h.hash)
                    .count();
                let better = best.as_ref().is_none_or(|(b, _)| {
                    (h.version, &h.hash) > (b.local.head().version, &b.local.head().hash)
                });
                if better {
                    best = Some((v.clone(), agreed));
                }
            }
        }
        let Some((v, agreed)) = best else {
            tracing::info!(%record, "191: {} (no proof path verifies from its pin)", rosters::NOT_VERIFIED);
            return Vec::new();
        };
        let h = v.local.head();
        if !rosters::adopt((h.version, h.hash.as_str()), peers, agreed) {
            tracing::warn!(%record, version = h.version, "191: {} (a verified head no other machine holds)", rosters::NOT_VERIFIED);
            return Vec::new();
        }
        tracing::info!(%record, version = h.version, members = h.members.len(), "191: contact roster bootstrapped from its pin");
        vec![
            records::local_row(record, &v.local),
            records::head_row(record, me, &v.local, false),
        ]
    }

    /// A record with state: compare with every live machine's head and act (D1).
    #[allow(clippy::too_many_arguments)]
    async fn diverge(
        &self,
        record: &str,
        me: &str,
        mine: rosters::Local,
        pins: &BTreeSet<String>,
        peers: &[PeerHead],
        hints: &[String],
        row: Option<&PeerHead>,
        family: &BTreeSet<String>,
    ) -> Vec<RecordWrite> {
        let mut l = mine.clone();
        let mut conflict = false;
        let mut flips = self.flip_budget(record);
        for p in peers {
            let key = (record.to_owned(), p.machine.clone());
            let seen = rosters::compare(&l, p);
            let fetch = match seen {
                Seen::Agree | Seen::Ahead | Seen::Theirs => {
                    self.rosters.lock().behind.remove(&key);
                    None
                }
                Seen::Behind => {
                    let mut m = self.rosters.lock();
                    let n = {
                        let n = m.behind.entry(key.clone()).or_default();
                        *n += 1;
                        *n
                    };
                    if n >= rosters::STALE_REFRESHES {
                        m.behind.remove(&key);
                        Some(None)
                    } else {
                        None
                    }
                }
                Seen::Fetch(to) => Some(Some(to)),
            };
            let Some(to) = fetch else { continue };
            let trusted = l.head().members.clone();
            let targets = dial_order(trusted.iter().chain(pins).chain(hints), family);
            let got = self
                .paths(&targets, &l.chain, pins, to.map(|v| (v, p.hash.as_str())))
                .await;
            let mut contents: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
            for (v, copies) in &got {
                for (k, s) in &v.contents {
                    contents.entry(*k).or_default().extend(s.iter().cloned());
                }
                rosters::advance(&mut l, copies, &mut flips, None);
            }
            if let Some(to) = to {
                let h = l.head();
                if (h.version, h.hash.as_str()) != (to, p.hash.as_str())
                    && rosters::conflict(&contents, to, &p.hash)
                {
                    tracing::warn!(%record, machine = %p.machine, version = to, "191: {}", rosters::DISAGREE);
                    conflict = true;
                }
            }
        }
        self.spend_flip(record, flips);
        let mut w = Vec::new();
        if l != mine {
            w.push(records::local_row(record, &l));
        }
        let h = l.head();
        let same = row.is_some_and(|r| {
            (r.version, r.hash.as_str()) == (h.version, h.hash.as_str()) && r.conflict == conflict
        });
        // A head that moved, a flag that changed, a flag kept (written again so it does not
        // expire), or no row yet.
        if !same || conflict {
            w.push(records::head_row(record, me, &l, conflict));
        }
        w
    }
}

/// Whom to dial, first come first, never a machine of this mesh.
fn dial_order<'a>(
    eids: impl Iterator<Item = &'a String>,
    family: &BTreeSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in eids {
        if !family.contains(e) && !out.contains(e) && e.parse::<iroh::EndpointId>().is_ok() {
            out.push(e.clone());
        }
    }
    out
}
