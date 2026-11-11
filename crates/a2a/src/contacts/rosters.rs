//! 191 (D1, P2): signed rosters, as pure functions over signed copies, so every machine
//! computes the same result from the same copies.
//! - A copy is `{chain, version, members, signer, sig}`; `sig` is the signer's Ed25519
//!   signature over [`DOMAIN`] followed by the JCS of `{chain, version, members, signer}`.
//! - Our side: [`membership`] (the cap), [`decide`] (sign, co-sign or nothing), [`hops`] (the
//!   proof path), [`keep`] (what pruning keeps), [`carried`] (the card extension).
//! - The contact's side: [`base`], [`advance`] (jump and replay, the equal-version switch),
//!   [`verify_path`], [`adopt`], [`compare`] and [`conflict`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

/// The roster signature's domain prefix (D1): it separates it from the card JWS the same
/// key signs.
pub const DOMAIN: &str = "theuth/a2a/roster/v1\n";
/// The card extension carrying our chains (and the body of a `NOTICE_PATH` push).
pub const EXT_MEMBERS: &str = "https://theuth.io/a2a/ext/members/v1";
/// The contact-only proof-path route.
pub const ROSTER_PATH: &str = "/theuth/roster-path";
/// Most members of one roster; a contact refuses a larger one.
pub const MEMBERS_MAX: usize = 256;
/// Versions per chain a card (and a push) carries.
pub const CARRIED: usize = 8;
/// Signed copies kept and carried per content of a version.
pub const SIGS_MAX: usize = 16;
/// Accepted history a contact machine keeps per record (and publishes as `[version, hash]`).
pub const HISTORY_MAX: usize = 64;
/// Hops in one proof-path answer.
pub const PATH_HOPS_MAX: usize = 64;
/// Bytes in one proof-path answer.
pub const PATH_BYTES_MAX: usize = 256 * 1024;
/// A `NOTICE_PATH` body above this is refused with 413.
pub const NOTICE_BYTES_MAX: usize = 192 * 1024;
/// Chains read from one payload.
pub const CHAINS_MAX: usize = 16;
/// Version entries read per chain from one payload (a path holds up to 64 hops, each with
/// every content signed there).
pub const ENTRIES_MAX: usize = 4 * PATH_HOPS_MAX;
/// A contact machine still behind a peer after this many refreshes fetches a proof path.
pub const STALE_REFRESHES: u32 = 3;
/// A peer's `conflict` not written again within this many refreshes expires.
pub const CONFLICT_REFRESHES: u64 = 3;
/// Our machines a contact machine asks for a proof path when that many are reachable.
pub const SOURCES_MIN: usize = 2;
/// The problem of a record this machine cannot (yet) verify the other side's machines of.
pub const NOT_VERIFIED: &str = "cannot verify this contact's machines";
/// The problem of a record whose machines here hold different rosters of the other side.
pub const DISAGREE: &str = "this contact's machines disagree about yours; remove and pair again";

/// The inflation alert's text (alert only).
pub fn inflated(contact: &str) -> String {
    format!(
        "a machine of {contact} follows a roster we never signed; one of ours may be compromised"
    )
}

/// How often a contact machine refreshes its rosters (proof paths, divergence, its heads
/// row): the card refresh's `OFFERS_TTL`.
pub const ROUND: std::time::Duration = std::time::Duration::from_secs(60);

/// One signed copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copy {
    /// The signing side's record id.
    pub chain: String,
    /// The counter in that chain.
    pub version: u64,
    /// The signing mesh's membership, sorted, unique.
    pub members: Vec<String>,
    /// The signer's endpoint id (one of `members`).
    pub signer: String,
    /// base64url (no padding) of the 64-byte signature.
    pub sig: String,
}

fn content_json(chain: &str, version: u64, members: &[String]) -> serde_json::Value {
    serde_json::json!({ "chain": chain, "version": version, "members": members })
}

/// The hex SHA-256 of the JCS of `{chain, version, members}`: what ties are broken on.
pub fn content_hash(chain: &str, version: u64, members: &[String]) -> String {
    let jcs = super::jcs(&content_json(chain, version, members));
    data_encoding::HEXLOWER.encode(&sha2::Sha256::digest(jcs.as_bytes()))
}

fn signing_input(chain: &str, version: u64, members: &[String], signer: &str) -> Vec<u8> {
    let v = serde_json::json!({
        "chain": chain, "version": version, "members": members, "signer": signer,
    });
    let mut out = DOMAIN.as_bytes().to_vec();
    out.extend_from_slice(super::jcs(&v).as_bytes());
    out
}

/// Sign `members` (sorted here) at `version` of `chain` as `signer`.
pub fn sign(
    chain: &str,
    version: u64,
    members: &[String],
    signer: &str,
    sign: &dyn Fn(&[u8]) -> Vec<u8>,
) -> Copy {
    let mut members = members.to_vec();
    members.sort();
    members.dedup();
    let sig = sign(&signing_input(chain, version, &members, signer));
    Copy {
        chain: chain.to_owned(),
        version,
        members,
        signer: signer.to_owned(),
        sig: data_encoding::BASE64URL_NOPAD.encode(&sig),
    }
}

impl Copy {
    /// Its content hash.
    pub fn hash(&self) -> String {
        content_hash(&self.chain, self.version, &self.members)
    }

    /// The copy verifies: a chain, a version from 1, 1 to [`MEMBERS_MAX`] sorted unique
    /// endpoint ids, the signer among them, and a valid signature under [`DOMAIN`].
    pub fn verify(&self) -> Result<(), String> {
        if self.chain.is_empty() || self.chain.len() > 64 || self.version == 0 {
            return Err("malformed chain or version".into());
        }
        if self.members.is_empty() || self.members.len() > MEMBERS_MAX {
            return Err(format!(
                "{} members (1 to {MEMBERS_MAX})",
                self.members.len()
            ));
        }
        if self.members.windows(2).any(|w| w[0] >= w[1]) {
            return Err("members not sorted and unique".into());
        }
        if self
            .members
            .iter()
            .any(|m| m.parse::<iroh::EndpointId>().is_err())
        {
            return Err("a member is not an endpoint id".into());
        }
        if !self.members.contains(&self.signer) {
            return Err("the signer is not a member".into());
        }
        let key: iroh::EndpointId = self
            .signer
            .parse()
            .map_err(|_| "the signer is not an endpoint id".to_owned())?;
        let bytes: [u8; 64] = data_encoding::BASE64URL_NOPAD
            .decode(self.sig.as_bytes())
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or("the signature is not 64 bytes of base64url")?;
        key.verify(
            &signing_input(&self.chain, self.version, &self.members, &self.signer),
            &iroh::Signature::from_bytes(&bytes),
        )
        .map_err(|_| "the signature does not verify".to_owned())
    }
}

// ---------------------------------------------------------------------------
// The wire payload: the card extension, the push body and the proof path.
// ---------------------------------------------------------------------------

/// One signature of a carried content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sig {
    /// The signer.
    pub signer: String,
    /// Its signature.
    pub sig: String,
}

/// One content of a version with its signatures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// The version.
    pub version: u64,
    /// The members, sorted.
    pub members: Vec<String>,
    /// At most [`SIGS_MAX`] signatures.
    pub sigs: Vec<Sig>,
}

/// One chain as carried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    /// The chain (the record id on the signing side).
    pub chain: String,
    /// Its versions, oldest first, one entry per content.
    pub versions: Vec<Version>,
}

/// The extension's `params` and a push body: `{chains: [...]}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carried {
    /// Each chain carried.
    pub chains: Vec<Chain>,
}

/// A proof-path answer: hops from the pin's first signed version, oldest first; `next`
/// continues it (`from=<next>`) when it was cut at [`PATH_HOPS_MAX`] or [`PATH_BYTES_MAX`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathAnswer {
    /// The chain.
    pub chain: String,
    /// The hops, each with every qualifying content.
    pub versions: Vec<Version>,
    /// Where the next answer starts, when there is one.
    pub next: Option<u64>,
}

impl Chain {
    /// The signed copies this chain carries, read within bounds ([`ENTRIES_MAX`] entries,
    /// [`SIGS_MAX`] signatures each); nothing here is verified.
    pub fn copies(&self) -> Vec<Copy> {
        self.versions
            .iter()
            .take(ENTRIES_MAX)
            .flat_map(|v| {
                v.sigs.iter().take(SIGS_MAX).map(|s| Copy {
                    chain: self.chain.clone(),
                    version: v.version,
                    members: v.members.clone(),
                    signer: s.signer.clone(),
                    sig: s.sig.clone(),
                })
            })
            .collect()
    }
}

impl Carried {
    /// Every copy of every chain (at most [`CHAINS_MAX`]), unverified.
    pub fn copies(&self) -> Vec<Copy> {
        self.chains
            .iter()
            .take(CHAINS_MAX)
            .flat_map(Chain::copies)
            .collect()
    }
}

/// The copies that verify, each once.
pub fn verified(copies: impl IntoIterator<Item = Copy>) -> Vec<Copy> {
    let mut seen = BTreeSet::new();
    copies
        .into_iter()
        .filter(|c| seen.insert((c.chain.clone(), c.version, c.signer.clone(), c.sig.clone())))
        .filter(|c| c.verify().is_ok())
        .collect()
}

/// Group `copies` of one version into carried entries, one per content (highest hash
/// first), signatures in the order given, at most [`SIGS_MAX`] each.
fn entries(copies: &[&Copy]) -> Vec<Version> {
    let mut by: BTreeMap<String, Version> = BTreeMap::new();
    for c in copies {
        let e = by.entry(c.hash()).or_insert_with(|| Version {
            version: c.version,
            members: c.members.clone(),
            sigs: Vec::new(),
        });
        if e.sigs.len() < SIGS_MAX {
            e.sigs.push(Sig {
                signer: c.signer.clone(),
                sig: c.sig.clone(),
            });
        }
    }
    by.into_values().rev().collect()
}

// ---------------------------------------------------------------------------
// Our side.
// ---------------------------------------------------------------------------

/// The roster this mesh signs for a record: its members (`(eid, added_ms)`), capped at
/// [`MEMBERS_MAX`] (the record's pins first, then the oldest), sorted.
pub fn membership(members: &[(String, u64)], pins: &BTreeSet<String>) -> Vec<String> {
    let mut m = members.to_vec();
    m.sort_by(|a, b| (!pins.contains(&a.0), a.1, &a.0).cmp(&(!pins.contains(&b.0), b.1, &b.0)));
    m.dedup_by(|a, b| a.0 == b.0);
    let mut out: Vec<String> = m.into_iter().take(MEMBERS_MAX).map(|(e, _)| e).collect();
    out.sort();
    out.dedup();
    out
}

/// The latest content of a chain: the highest version, then the highest hash.
pub fn latest(copies: &[Copy]) -> Option<&Copy> {
    copies
        .iter()
        .max_by(|a, b| (a.version, a.hash()).cmp(&(b.version, b.hash())))
}

/// The order signatures are kept and carried in (D1): signers that are members of the
/// previous version first, then the pinned ones, then the rest, each oldest first.
pub fn sig_rank(
    signer: &str,
    prev: &BTreeSet<String>,
    pins: &BTreeSet<String>,
    added: &BTreeMap<String, u64>,
) -> (u8, u64, String) {
    let class = if prev.contains(signer) {
        0
    } else if pins.contains(signer) {
        1
    } else {
        2
    };
    (
        class,
        added.get(signer).copied().unwrap_or(u64::MAX),
        signer.to_owned(),
    )
}

/// The members of the latest content below `version` in `copies`.
fn prev_members(copies: &[Copy], version: u64) -> BTreeSet<String> {
    let below: Vec<Copy> = copies
        .iter()
        .filter(|c| c.version < version)
        .cloned()
        .collect();
    latest(&below)
        .map(|c| c.members.iter().cloned().collect())
        .unwrap_or_default()
}

/// What this machine signs now for a chain whose verified copies are `copies`, given the
/// membership it computes (D1): `None` when the latest content already is `mine` and it
/// signed it (or [`SIGS_MAX`] signers ranked before it did), else the version to sign
/// `mine` at (latest + 1 when they differ, the latest to co-sign).
pub fn decide(
    copies: &[Copy],
    mine: &[String],
    me: &str,
    pins: &BTreeSet<String>,
    added: &BTreeMap<String, u64>,
) -> Option<u64> {
    let Some(l) = latest(copies) else {
        return Some(1);
    };
    if l.members != mine {
        return Some(l.version + 1);
    }
    let hash = l.hash();
    let signers: Vec<&str> = copies
        .iter()
        .filter(|c| c.version == l.version && c.hash() == hash)
        .map(|c| c.signer.as_str())
        .collect();
    if signers.contains(&me) {
        return None;
    }
    let prev = prev_members(copies, l.version);
    let rank = |s: &str| sig_rank(s, &prev, pins, added);
    let ahead = signers.iter().filter(|s| rank(s) < rank(me)).count();
    (ahead < SIGS_MAX).then_some(l.version)
}

/// One hop of a proof path: its version, the content the path goes on from (highest hash),
/// and every copy there signed by a member of the previous hop (by the pin, for the first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    /// The version.
    pub version: u64,
    /// The members of the content the next hop must be signed from.
    pub roster: Vec<String>,
    /// The qualifying copies.
    pub copies: Vec<Copy>,
}

/// The minimal hop chain (D1) of verified `copies` of one chain from `pin`: its first signed
/// version, then greedily the highest version up to `to` a member of the current hop signed.
pub fn hops(copies: &[Copy], pin: &str, to: u64) -> Vec<Hop> {
    let mut out = Vec::new();
    let Some(first) = copies
        .iter()
        .filter(|c| c.signer == pin && c.version <= to)
        .map(|c| c.version)
        .min()
    else {
        return out;
    };
    let mut trusted: BTreeSet<String> = BTreeSet::from([pin.to_owned()]);
    let mut at = first;
    loop {
        let q: Vec<Copy> = copies
            .iter()
            .filter(|c| c.version == at && trusted.contains(&c.signer))
            .cloned()
            .collect();
        let Some(best) = latest(&q) else { break };
        let roster = best.members.clone();
        out.push(Hop {
            version: at,
            roster: roster.clone(),
            copies: q,
        });
        trusted = roster.into_iter().collect();
        let Some(next) = copies
            .iter()
            .filter(|c| c.version > at && c.version <= to && trusted.contains(&c.signer))
            .map(|c| c.version)
            .max()
        else {
            break;
        };
        at = next;
    }
    out
}

/// The versions pruning keeps (D1): every pin's hop chain, recomputed first, plus the last
/// [`CARRIED`] versions.
pub fn keep(copies: &[Copy], pins: &BTreeSet<String>) -> BTreeSet<u64> {
    let mut out: BTreeSet<u64> = pins
        .iter()
        .flat_map(|p| hops(copies, p, u64::MAX))
        .map(|h| h.version)
        .collect();
    let all: BTreeSet<u64> = copies.iter().map(|c| c.version).collect();
    out.extend(all.iter().rev().take(CARRIED));
    out
}

/// The copies of one (version, content) pruning keeps: the first [`SIGS_MAX`] in
/// [`sig_rank`] order. Returns the copies to drop.
pub fn over_cap<'a>(
    copies: &'a [Copy],
    pins: &BTreeSet<String>,
    added: &BTreeMap<String, u64>,
) -> Vec<&'a Copy> {
    let mut by: BTreeMap<(u64, String), Vec<&Copy>> = BTreeMap::new();
    for c in copies {
        by.entry((c.version, c.hash())).or_default().push(c);
    }
    let mut out = Vec::new();
    for ((version, _), mut list) in by {
        let prev = prev_members(copies, version);
        list.sort_by_key(|c| sig_rank(&c.signer, &prev, pins, added));
        out.extend(list.into_iter().skip(SIGS_MAX));
    }
    out
}

/// The card extension's entry for one chain (D1): its last [`CARRIED`] versions, each content
/// once with at most [`SIGS_MAX`] signatures, in [`sig_rank`] order.
pub fn carried(
    chain: &str,
    copies: &[Copy],
    pins: &BTreeSet<String>,
    added: &BTreeMap<String, u64>,
) -> Chain {
    let all: BTreeSet<u64> = copies.iter().map(|c| c.version).collect();
    let last: Vec<u64> = all.iter().rev().take(CARRIED).rev().copied().collect();
    let mut versions = Vec::new();
    for v in last {
        let prev = prev_members(copies, v);
        let mut at: Vec<&Copy> = copies.iter().filter(|c| c.version == v).collect();
        at.sort_by_key(|c| sig_rank(&c.signer, &prev, pins, added));
        versions.extend(entries(&at));
    }
    Chain {
        chain: chain.to_owned(),
        versions,
    }
}

/// A proof-path answer from `hops`, starting at the first hop at or after `from`, cut at
/// [`PATH_HOPS_MAX`] hops or [`PATH_BYTES_MAX`] bytes (always at least one hop past `from`
/// when there is one, so a continuation makes progress).
pub fn path_answer(chain: &str, hops: &[Hop], from: u64) -> PathAnswer {
    let rest: Vec<&Hop> = hops.iter().filter(|h| h.version >= from).collect();
    let mut out = PathAnswer {
        chain: chain.to_owned(),
        ..PathAnswer::default()
    };
    let mut bytes = 64 + chain.len();
    let mut taken = 0;
    for h in &rest {
        let refs: Vec<&Copy> = h.copies.iter().collect();
        let e = entries(&refs);
        let size: usize = e
            .iter()
            .map(|v| serde_json::to_vec(v).map_or(0, |b| b.len() + 1))
            .sum();
        let progress = taken < 2 && h.version > from;
        if taken >= PATH_HOPS_MAX || (bytes + size > PATH_BYTES_MAX && !progress) {
            break;
        }
        bytes += size;
        taken += 1;
        out.versions.extend(e);
    }
    if taken < rest.len() && taken > 0 {
        out.next = Some(rest[taken - 1].version);
    }
    out
}

// ---------------------------------------------------------------------------
// The contact's side: per machine, learned only from our signed copies.
// ---------------------------------------------------------------------------

/// One accepted version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The version.
    pub version: u64,
    /// Its content hash.
    pub hash: String,
    /// Its members.
    pub members: Vec<String>,
}

/// A machine's own verified state of the other side's chain for one record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    /// The chain it is based on.
    pub chain: String,
    /// The pin whose copy was the base.
    pub base: String,
    /// The verified signers of the head's content (attribution).
    pub signers: BTreeSet<String>,
    /// At most [`HISTORY_MAX`] accepted versions, the head last (never empty).
    pub history: Vec<Entry>,
}

impl Local {
    /// The current roster.
    pub fn head(&self) -> &Entry {
        self.history.last().expect("a local state has a head")
    }

    /// `(version, hash)` of every entry kept.
    pub fn pairs(&self) -> Vec<(u64, String)> {
        self.history
            .iter()
            .map(|e| (e.version, e.hash.clone()))
            .collect()
    }

    fn push(&mut self, e: Entry) {
        self.history.push(e);
        if self.history.len() > HISTORY_MAX {
            self.history.remove(0);
        }
    }
}

/// The base (D1) among verified `copies`: the first copy signed by one of `pins` (which
/// lists it: a signer is always a member); on the lowest chain when several qualify, at its
/// lowest such version, the highest hash there.
pub fn base(pins: &BTreeSet<String>, copies: &[Copy]) -> Option<Local> {
    let q: Vec<&Copy> = copies.iter().filter(|c| pins.contains(&c.signer)).collect();
    let chain = q.iter().map(|c| c.chain.as_str()).min()?.to_owned();
    let version = q
        .iter()
        .filter(|c| c.chain == chain)
        .map(|c| c.version)
        .min()?;
    let at: Vec<Copy> = q
        .iter()
        .filter(|c| c.chain == chain && c.version == version)
        .map(|c| (*c).clone())
        .collect();
    let best = latest(&at)?;
    let hash = best.hash();
    Some(Local {
        chain,
        base: best.signer.clone(),
        signers: at
            .iter()
            .filter(|c| c.hash() == hash)
            .map(|c| c.signer.clone())
            .collect(),
        history: vec![Entry {
            version,
            hash,
            members: best.members.clone(),
        }],
    })
}

/// The accept rule (D1) over verified `copies`: on the state's chain, take the lowest
/// version above the head that a member of the current roster signed (the highest hash
/// there), and repeat; at the head's version, a higher hash signed by a member of the roster
/// before the head or of the head's own replaces it, at most `flips` times. `seen` collects,
/// per version stepped to, every qualifying hash. Returns whether the head moved.
pub fn advance(
    l: &mut Local,
    copies: &[Copy],
    flips: &mut usize,
    mut seen: Option<&mut BTreeMap<u64, BTreeSet<String>>>,
) -> bool {
    let ok: Vec<&Copy> = copies.iter().filter(|c| c.chain == l.chain).collect();
    let mut moved = false;
    loop {
        let head = l.head().clone();
        let trusted: BTreeSet<&str> = head.members.iter().map(String::as_str).collect();
        let next = ok
            .iter()
            .filter(|c| c.version > head.version && trusted.contains(c.signer.as_str()))
            .map(|c| c.version)
            .min();
        if let Some(w) = next {
            let q: Vec<Copy> = ok
                .iter()
                .filter(|c| c.version == w && trusted.contains(c.signer.as_str()))
                .map(|c| (*c).clone())
                .collect();
            if let Some(s) = seen.as_deref_mut() {
                s.entry(w).or_default().extend(q.iter().map(Copy::hash));
            }
            let best = latest(&q).expect("a qualifying copy at the next version");
            l.push(Entry {
                version: w,
                hash: best.hash(),
                members: best.members.clone(),
            });
            l.signers.clear();
            moved = true;
            continue;
        }
        if *flips > 0 {
            let before = l.history.len().checked_sub(2).map(|i| &l.history[i]);
            let allowed: BTreeSet<&str> = before
                .into_iter()
                .flat_map(|e| e.members.iter())
                .chain(head.members.iter())
                .map(String::as_str)
                .collect();
            let q: Vec<Copy> = ok
                .iter()
                .filter(|c| c.version == head.version && allowed.contains(c.signer.as_str()))
                .map(|c| (*c).clone())
                .collect();
            if let Some(s) = seen.as_deref_mut() {
                s.entry(head.version)
                    .or_default()
                    .extend(q.iter().map(Copy::hash));
            }
            if let Some(best) = latest(&q).filter(|b| b.hash() > head.hash) {
                let last = l.history.len() - 1;
                l.history[last] = Entry {
                    version: head.version,
                    hash: best.hash(),
                    members: best.members.clone(),
                };
                l.signers.clear();
                *flips -= 1;
                moved = true;
                continue;
            }
        }
        break;
    }
    let head = l.head().clone();
    l.signers.extend(
        ok.iter()
            .filter(|c| c.version == head.version && c.hash() == head.hash)
            .map(|c| c.signer.clone()),
    );
    moved
}

/// A proof path verified from `pins` alone: the state it reaches and, per version stepped
/// to, every qualifying content hash.
#[derive(Clone, Debug)]
pub struct Verified {
    /// The state the path reaches.
    pub local: Local,
    /// Version → the qualifying content hashes there.
    pub contents: BTreeMap<u64, BTreeSet<String>>,
}

/// Verify a proof path (verified `copies` of `chain`) from the record's pins: its base, then
/// the accept rule hop by hop (no equal-version flips: every content of a hop is recorded).
pub fn verify_path(pins: &BTreeSet<String>, chain: &str, copies: &[Copy]) -> Option<Verified> {
    let on: Vec<Copy> = copies
        .iter()
        .filter(|c| c.chain == chain)
        .cloned()
        .collect();
    let mut local = base(pins, &on)?;
    let mut contents: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    let first = local.head().version;
    contents.entry(first).or_default().extend(
        on.iter()
            .filter(|c| c.version == first && pins.contains(&c.signer))
            .map(Copy::hash),
    );
    advance(&mut local, &on, &mut 0, Some(&mut contents));
    Some(Verified { local, contents })
}

/// Another machine's published head of the same record (its `contact_roster_heads` row).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerHead {
    /// That machine's endpoint id.
    pub machine: String,
    /// Its head.
    pub version: u64,
    /// Its head's hash.
    pub hash: String,
    /// Its last [`HISTORY_MAX`] `(version, hash)` pairs.
    pub history: Vec<(u64, String)>,
    /// It flagged a conflict.
    pub conflict: bool,
    /// When it last wrote the row.
    pub at_ms: i64,
    /// Untrusted dial hints: our members at its head.
    pub members: Vec<String>,
    /// Untrusted: the chain it follows.
    pub chain: String,
}

/// Fresh-machine adoption (D1 note 3): a verified head is adopted only if it equals a peer's
/// head, appears in a peer's history, or `agreed` (≥ 2) independent sources of ours agree on
/// it; with no peers at all the verified path is used.
pub fn adopt(head: (u64, &str), peers: &[PeerHead], agreed: usize) -> bool {
    peers.is_empty()
        || agreed >= SOURCES_MIN
        || peers.iter().any(|p| {
            (p.version, p.hash.as_str()) == head
                || p.history.iter().any(|(v, h)| (*v, h.as_str()) == head)
        })
}

/// How this machine's head relates to a peer's (D1 divergence detection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The same head.
    Agree,
    /// Ours is lower and in their history: merely stale (catch up).
    Behind,
    /// Theirs is lower and in our history: they catch up.
    Ahead,
    /// Otherwise, ours the lower (or equal): fetch a proof path up to this version.
    Fetch(u64),
    /// Otherwise, theirs the lower: they fetch.
    Theirs,
}

/// Compare `mine` with `peer`'s head.
pub fn compare(mine: &Local, peer: &PeerHead) -> Seen {
    let h = mine.head();
    if (h.version, h.hash.as_str()) == (peer.version, peer.hash.as_str()) {
        return Seen::Agree;
    }
    if h.version < peer.version
        && peer
            .history
            .iter()
            .any(|(v, x)| (*v, x.as_str()) == (h.version, h.hash.as_str()))
    {
        return Seen::Behind;
    }
    if peer.version < h.version
        && mine
            .history
            .iter()
            .any(|e| (e.version, e.hash.as_str()) == (peer.version, peer.hash.as_str()))
    {
        return Seen::Ahead;
    }
    if h.version <= peer.version {
        Seen::Fetch(peer.version)
    } else {
        Seen::Theirs
    }
}

/// A conflict (D1): a verified path reached `to` and `theirs` is none of its contents there.
/// A path that stops short is never one.
pub fn conflict(contents: &BTreeMap<u64, BTreeSet<String>>, to: u64, theirs: &str) -> bool {
    contents.get(&to).is_some_and(|s| !s.contains(theirs))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Key(iroh::SecretKey);

    impl Key {
        fn new() -> Self {
            Self(iroh::SecretKey::generate())
        }
        fn eid(&self) -> String {
            self.0.public().to_string()
        }
        fn sign(&self, chain: &str, version: u64, members: &[&Key]) -> Copy {
            let m: Vec<String> = members.iter().map(|k| k.eid()).collect();
            sign(chain, version, &m, &self.eid(), &|b| {
                self.0.sign(b).to_bytes().to_vec()
            })
        }
    }

    fn set(keys: &[&Key]) -> BTreeSet<String> {
        keys.iter().map(|k| k.eid()).collect()
    }

    fn local_at(chain: &str, version: u64, members: &[&Key]) -> Local {
        let mut m: Vec<String> = members.iter().map(|k| k.eid()).collect();
        m.sort();
        Local {
            chain: chain.into(),
            base: members[0].eid(),
            signers: BTreeSet::new(),
            history: vec![Entry {
                version,
                hash: content_hash(chain, version, &m),
                members: m,
            }],
        }
    }

    /// The accept rule refuses another record's chain, a signer outside the current roster,
    /// an equal or lower version, a bad signature, one made without the domain prefix (the
    /// card JWS input, say) and a roster over 256 members; a valid next version is taken.
    #[test]
    fn the_accept_rule_refuses_everything_it_cannot_prove() {
        let (n1, n2, x) = (Key::new(), Key::new(), Key::new());
        let mut l = local_at("chain-a", 3, &[&n1, &n2]);
        let refused = |l: &mut Local, c: Copy| {
            let before = l.clone();
            let ok = verified([c]);
            // No equal-version flip allowed here: that rule has its own test.
            assert!(!advance(l, &ok, &mut 0, None));
            assert_eq!(*l, before);
        };
        refused(&mut l, n1.sign("chain-b", 4, &[&n1]));
        refused(&mut l, x.sign("chain-a", 4, &[&x]));
        refused(&mut l, n1.sign("chain-a", 3, &[&n1]));
        refused(&mut l, n1.sign("chain-a", 2, &[&n1]));
        let mut bad = n1.sign("chain-a", 4, &[&n1]);
        bad.members = vec![n2.eid()].into_iter().chain([n1.eid()]).collect();
        bad.members.sort();
        refused(&mut l, bad);
        // The same key over the same JCS without the prefix.
        let m = vec![n1.eid()];
        let body =
            serde_json::json!({"chain": "chain-a", "version": 4, "members": m, "signer": n1.eid()});
        let raw = n1.0.sign(super::super::jcs(&body).as_bytes());
        refused(
            &mut l,
            Copy {
                chain: "chain-a".into(),
                version: 4,
                members: m.clone(),
                signer: n1.eid(),
                sig: data_encoding::BASE64URL_NOPAD.encode(&raw.to_bytes()),
            },
        );
        // 257 members.
        let many: Vec<Key> = (0..MEMBERS_MAX).map(|_| Key::new()).collect();
        let mut refs: Vec<&Key> = many.iter().collect();
        refs.push(&n1);
        let big = n1.sign("chain-a", 4, &refs);
        assert_eq!(big.members.len(), MEMBERS_MAX + 1);
        assert!(big.verify().is_err());
        refused(&mut l, big);
        // A good one moves it.
        let good = verified([n2.sign("chain-a", 4, &[&n2])]);
        assert!(advance(&mut l, &good, &mut 0, None));
        assert_eq!(l.head().version, 4);
        assert_eq!(l.head().members, vec![n2.eid()]);
    }

    /// At one version the highest content hash wins among copies the current roster signed.
    #[test]
    fn the_higher_hash_wins_at_one_version() {
        let (n1, n2, n5, n6) = (Key::new(), Key::new(), Key::new(), Key::new());
        let mut l = local_at("c", 1, &[&n1, &n2]);
        let a = n1.sign("c", 2, &[&n1, &n2, &n5]);
        let b = n2.sign("c", 2, &[&n1, &n2, &n6]);
        let win = a.hash().max(b.hash());
        assert!(advance(&mut l, &verified([a, b]), &mut 0, None));
        assert_eq!(l.head().hash, win);
    }

    /// A contact at v1 offered only v4..v11 (v4 signed by an old member) jumps to v4 and
    /// replays to v11, each step signed from the roster current at that step; offered only
    /// copies by machines it does not trust, it stays at v1.
    #[test]
    fn jump_then_replay_and_untrusted_copies_change_nothing() {
        let (n1, n2) = (Key::new(), Key::new());
        let n5: Vec<Key> = (0..12).map(|_| Key::new()).collect();
        let mut copies = Vec::new();
        for v in 4..=11u64 {
            let extra = &n5[v as usize];
            // v4 signed by n2 (in v1's roster); later ones by whoever the previous one lists.
            copies.push(n2.sign("c", v, &[&n1, &n2, extra]));
        }
        let mut l = local_at("c", 1, &[&n1, &n2]);
        let untrusted: Vec<Copy> = (4..=11u64).map(|v| n5[0].sign("c", v, &[&n5[0]])).collect();
        assert!(!advance(&mut l, &verified(untrusted), &mut 1, None));
        assert_eq!(l.head().version, 1);
        assert!(advance(&mut l, &verified(copies), &mut 0, None));
        assert_eq!(l.head().version, 11);
        assert!(l.head().members.contains(&n5[11].eid()));
        // A gap the roster cannot bridge: v12 signed only by v10's extra (not in v11).
        let gap = verified([n5[10].sign("c", 12, &[&n5[10]])]);
        assert!(!advance(&mut l, &gap, &mut 0, None));
        assert_eq!(l.head().version, 11);
    }

    /// Equal version: a higher hash signed by a member of the roster before the head (or the
    /// head's own) replaces it, at most `flips` times; a lower hash never does.
    #[test]
    fn equal_version_replacement_is_capped_per_refresh() {
        let (n1, n2, n5, n6, n7) = (Key::new(), Key::new(), Key::new(), Key::new(), Key::new());
        let v1 = n1.sign("c", 1, &[&n1, &n2]);
        let mut contents: Vec<Copy> = [&n5, &n6, &n7]
            .iter()
            .map(|k| n2.sign("c", 2, &[&n1, &n2, k]))
            .collect();
        contents.sort_by_key(Copy::hash);
        let (low, mid, high) = (&contents[0], &contents[1], &contents[2]);
        let mut l = base(&set(&[&n1]), &verified([v1.clone()])).unwrap();
        assert!(advance(&mut l, &verified([low.clone()]), &mut 0, None));
        assert_eq!(l.head().hash, low.hash());
        // Two higher contents arrive; one flip allowed this refresh: it takes the highest.
        let mut flips = 1;
        assert!(advance(
            &mut l,
            &verified([mid.clone(), high.clone()]),
            &mut flips,
            None
        ));
        assert_eq!((l.head().hash.clone(), flips), (high.hash(), 0));
        // No flip left: a still higher one waits (none here); a lower one never wins.
        assert!(!advance(&mut l, &verified([low.clone()]), &mut 1, None));
        assert_eq!(l.head().hash, high.hash());
        assert_eq!(
            l.history.len(),
            2,
            "a flip replaces the head, it adds no entry"
        );
        // Flip budget 0 blocks a higher one.
        let mut l2 = base(&set(&[&n1]), &verified([v1])).unwrap();
        advance(&mut l2, &verified([low.clone()]), &mut 0, None);
        assert!(!advance(&mut l2, &verified([high.clone()]), &mut 0, None));
        assert_eq!(l2.head().hash, low.hash());
    }

    /// Co-signing: a member whose membership equals the latest content co-signs it once;
    /// one whose view differs signs v + 1; nothing once signed. The 16-copy cap stops a
    /// member ranked after 16 signers.
    #[test]
    fn every_member_co_signs_the_latest_once() {
        let (n1, n2, n3) = (Key::new(), Key::new(), Key::new());
        let m = |ks: &[&Key]| {
            let mut v: Vec<String> = ks.iter().map(|k| k.eid()).collect();
            v.sort();
            v
        };
        let pins = set(&[&n1]);
        let added = BTreeMap::new();
        let all = m(&[&n1, &n2, &n3]);
        assert_eq!(decide(&[], &all, &n1.eid(), &pins, &added), Some(1));
        let v1 = verified([n1.sign("c", 1, &[&n1, &n2, &n3])]);
        assert_eq!(
            decide(&v1, &all, &n2.eid(), &pins, &added),
            Some(1),
            "co-sign"
        );
        assert_eq!(decide(&v1, &all, &n1.eid(), &pins, &added), None, "signed");
        let fewer = m(&[&n1, &n2]);
        assert_eq!(
            decide(&v1, &fewer, &n2.eid(), &pins, &added),
            Some(2),
            "v + 1"
        );
        // Cap: 16 older signers already signed; a newcomer does not co-sign.
        let old: Vec<Key> = (0..SIGS_MAX).map(|_| Key::new()).collect();
        let newcomer = Key::new();
        let mut refs: Vec<&Key> = old.iter().collect();
        refs.push(&newcomer);
        let members = m(&refs);
        let added: BTreeMap<String, u64> = old
            .iter()
            .enumerate()
            .map(|(i, k)| (k.eid(), i as u64))
            .chain([(newcomer.eid(), 1_000)])
            .collect();
        let copies: Vec<Copy> = verified(old.iter().map(|k| k.sign("c", 1, &refs)));
        assert_eq!(
            decide(&copies, &members, &newcomer.eid(), &BTreeSet::new(), &added),
            None
        );
        assert_eq!(
            decide(
                &copies[1..],
                &members,
                &newcomer.eid(),
                &BTreeSet::new(),
                &added
            ),
            Some(1)
        );
    }

    /// A legacy (pin-only) record bases on the copy its pin signed, not on another member's;
    /// with copies of two chains it takes the lowest.
    #[test]
    fn the_base_is_the_pins_copy_on_the_lowest_chain() {
        let (n1, n2) = (Key::new(), Key::new());
        let pins = set(&[&n1]);
        assert!(base(&pins, &verified([n2.sign("b", 1, &[&n1, &n2])])).is_none());
        let l = base(
            &pins,
            &verified([
                n2.sign("b", 1, &[&n1, &n2]),
                n1.sign("b", 1, &[&n1, &n2]),
                n1.sign("a", 3, &[&n1, &n2]),
            ]),
        )
        .unwrap();
        assert_eq!((l.chain.as_str(), l.head().version), ("a", 3));
        assert_eq!(l.base, n1.eid());
    }

    /// The cap: 300 members are signed as 256, the pins first, then the oldest; a removal of
    /// one of them still changes the roster.
    #[test]
    fn a_membership_over_256_is_capped_pins_first_then_oldest() {
        let keys: Vec<Key> = (0..300).map(|_| Key::new()).collect();
        let members: Vec<(String, u64)> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.eid(), i as u64))
            .collect();
        let pins = BTreeSet::from([keys[299].eid()]);
        let m = membership(&members, &pins);
        assert_eq!(m.len(), MEMBERS_MAX);
        assert!(m.contains(&keys[299].eid()), "the pin, though newest");
        assert!(m.contains(&keys[0].eid()) && m.contains(&keys[254].eid()));
        assert!(
            !m.contains(&keys[255].eid()),
            "the newest non-pins left out"
        );
        let without: Vec<(String, u64)> = members[1..].to_vec();
        let m2 = membership(&without, &pins);
        assert!(!m2.contains(&keys[0].eid()));
        assert!(m2.contains(&keys[255].eid()));
        assert_ne!(m, m2);
    }

    /// Carried signatures: members of the previous version first (oldest first), then the
    /// pinned signers, then the rest, at most 16: a member trusted at v3 is never crowded out
    /// by 20 newcomers, and a contact at v1 reaches v11.
    #[test]
    fn carried_signatures_keep_old_members_first() {
        let (n1, n2) = (Key::new(), Key::new());
        let newcomers: Vec<Key> = (0..20).map(|_| Key::new()).collect();
        let mut added: BTreeMap<String, u64> = BTreeMap::from([(n1.eid(), 5), (n2.eid(), 6)]);
        for (i, k) in newcomers.iter().enumerate() {
            added.insert(k.eid(), 100 + i as u64);
        }
        let mut copies = Vec::new();
        copies.push(n1.sign("c", 3, &[&n1, &n2]));
        let mut v4: Vec<&Key> = newcomers.iter().collect();
        v4.push(&n2);
        for k in &newcomers {
            copies.push(k.sign("c", 4, &v4));
        }
        copies.push(n2.sign("c", 4, &v4));
        for v in 5..=11 {
            copies.push(n2.sign("c", v, &v4));
        }
        let copies = verified(copies);
        let chain = carried("c", &copies, &BTreeSet::new(), &added);
        assert_eq!(chain.versions.len(), CARRIED);
        let at4 = chain.versions.iter().find(|v| v.version == 4).unwrap();
        assert_eq!(at4.sigs.len(), SIGS_MAX);
        assert_eq!(at4.sigs[0].signer, n2.eid(), "the v3 member first");
        let mut l = local_at("c", 1, &[&n1, &n2]);
        assert!(advance(&mut l, &verified(chain.copies()), &mut 0, None));
        assert_eq!(l.head().version, 11);
        // Pinned signers come after previous members, before the rest.
        let pins = BTreeSet::from([newcomers[19].eid()]);
        let chain = carried("c", &copies, &pins, &added);
        let at4 = chain.versions.iter().find(|v| v.version == 4).unwrap();
        assert_eq!(
            (at4.sigs[0].signer.as_str(), at4.sigs[1].signer.as_str()),
            (n2.eid().as_str(), newcomers[19].eid().as_str())
        );
    }

    /// The proof path and pruning: the hop chain from the pin survives the 8-version window
    /// (recomputed before pruning), a hop not signed by a member of the previous hop breaks
    /// the path, and the answer is cut at 64 hops with `from` continuing it.
    #[test]
    fn proof_paths_hop_from_the_pin_and_survive_pruning() {
        let (n1, n2) = (Key::new(), Key::new());
        let mut copies = vec![n1.sign("c", 1, &[&n1, &n2])];
        for v in 2..=200u64 {
            copies.push(n2.sign("c", v, &[&n2]));
        }
        let copies = verified(copies);
        let pins = set(&[&n1]);
        let h = hops(&copies, &n1.eid(), u64::MAX);
        assert_eq!(
            h.iter().map(|h| h.version).collect::<Vec<_>>(),
            vec![1, 200],
            "greedy: v1 (the pin's), then the highest n2 signed"
        );
        let kept = keep(&copies, &pins);
        assert!(kept.contains(&1) && kept.contains(&200) && kept.contains(&193));
        assert!(!kept.contains(&100));
        let pruned: Vec<Copy> = copies
            .iter()
            .filter(|c| kept.contains(&c.version))
            .cloned()
            .collect();
        let v = verify_path(&pins, "c", &pruned).unwrap();
        assert_eq!(v.local.head().version, 200);
        // `to` stops it short.
        let short = hops(&pruned, &n1.eid(), 150);
        assert_eq!(short.last().map(|h| h.version), Some(1));
        // A broken hop: v2 signed by a stranger; the path stays at v1.
        let x = Key::new();
        let broken = verified([n1.sign("c", 1, &[&n1]), x.sign("c", 2, &[&x])]);
        let v = verify_path(&pins, "c", &broken).unwrap();
        assert_eq!(v.local.head().version, 1);
        // Bounds: a chain where every version names the next signer: 100 hops, cut at 64.
        let ks: Vec<Key> = (0..101).map(|_| Key::new()).collect();
        let mut chain = Vec::new();
        for v in 1..=100u64 {
            let (me, next) = (&ks[v as usize - 1], &ks[v as usize]);
            chain.push(me.sign("c", v, &[me, next]));
        }
        let chain = verified(chain);
        let h = hops(&chain, &ks[0].eid(), u64::MAX);
        assert_eq!(h.len(), 100);
        let first = path_answer("c", &h, 0);
        assert_eq!(first.versions.len(), PATH_HOPS_MAX);
        let next = first.next.unwrap();
        let rest = path_answer("c", &h, next);
        assert_eq!(rest.next, None);
        let mut all: Vec<Copy> = Chain {
            chain: "c".into(),
            versions: first.versions,
        }
        .copies();
        all.extend(
            Chain {
                chain: "c".into(),
                versions: rest.versions,
            }
            .copies(),
        );
        let v = verify_path(&BTreeSet::from([ks[0].eid()]), "c", &verified(all)).unwrap();
        assert_eq!(v.local.head().version, 100);
        // Size: every answer stays under 256 KiB.
        let big: Vec<Key> = (0..MEMBERS_MAX - 1).map(|_| Key::new()).collect();
        let mut wide = Vec::new();
        for v in 1..=40u64 {
            let mut m: Vec<&Key> = big.iter().collect();
            m.push(&n1);
            wide.push(n1.sign("c", v, &m));
        }
        let wide = verified(wide);
        let h: Vec<Hop> = (1..=40u64)
            .map(|v| Hop {
                version: v,
                roster: Vec::new(),
                copies: wide.iter().filter(|c| c.version == v).cloned().collect(),
            })
            .collect();
        let a = path_answer("c", &h, 0);
        assert!(serde_json::to_vec(&a).unwrap().len() <= PATH_BYTES_MAX);
        assert!(a.next.is_some());
    }

    /// Adoption: a fresh machine adopts a verified head equal to a peer's, in a peer's
    /// history, or agreed by two sources; else nothing (with no peers, its path).
    #[test]
    fn a_fresh_machine_adopts_only_a_corroborated_head() {
        let peer = PeerHead {
            machine: "c1".into(),
            version: 9,
            hash: "h9".into(),
            history: vec![(8, "h8".into()), (9, "h9".into())],
            ..PeerHead::default()
        };
        assert!(adopt((9, "h9"), std::slice::from_ref(&peer), 1));
        assert!(adopt((8, "h8"), std::slice::from_ref(&peer), 1));
        assert!(!adopt((20, "rogue"), std::slice::from_ref(&peer), 1));
        assert!(adopt((20, "x"), std::slice::from_ref(&peer), 2));
        assert!(adopt((20, "rogue"), &[], 1), "no peers: the limit class");
    }

    /// Divergence: a lower head in the other's history is stale; otherwise the lower machine
    /// fetches; a conflict needs a verified path that reaches the other head's version with
    /// other contents there, never one that stops short.
    #[test]
    fn divergence_needs_a_path_that_reaches_the_other_head() {
        let (n1, n2) = (Key::new(), Key::new());
        let mine = local_at("c", 9, &[&n2]);
        let h = mine.head().clone();
        let peer = |v: u64, hash: &str, hist: Vec<(u64, String)>| PeerHead {
            version: v,
            hash: hash.into(),
            history: hist,
            ..PeerHead::default()
        };
        assert_eq!(compare(&mine, &peer(9, &h.hash, vec![])), Seen::Agree);
        assert_eq!(
            compare(&mine, &peer(11, "h11", vec![(9, h.hash.clone())])),
            Seen::Behind
        );
        assert_eq!(compare(&mine, &peer(10, "rogue", vec![])), Seen::Fetch(10));
        assert_eq!(compare(&mine, &peer(9, "other", vec![])), Seen::Fetch(9));
        assert_eq!(compare(&mine, &peer(5, "old", vec![])), Seen::Theirs);
        let _ = n1;
        let mut contents = BTreeMap::new();
        contents.insert(9u64, BTreeSet::from([h.hash.clone()]));
        assert!(!conflict(&contents, 10, "rogue"), "stopped short of 10");
        contents.insert(10u64, BTreeSet::from(["honest".to_owned()]));
        assert!(conflict(&contents, 10, "rogue"));
        assert!(!conflict(&contents, 10, "honest"));
    }
}
