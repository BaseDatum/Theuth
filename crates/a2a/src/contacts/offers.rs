//! 183 F1 (§7.7): what each contact shares with this engine. A contact's card, fetched over
//! `theuth/a2a/1` ([`CARD_PATH`]), lists as skills the exports it grants THIS reader
//! ([`granted`]); the public card lists none. This engine keeps each contact's latest list
//! ([`Cached`]), refreshed at pairing, at boot, on the contact's [`NOTICE_PATH`] notice (sent
//! when its grants for us change) and, at most once per [`OFFERS_TTL`], when
//! `discover_peers` runs; the agent turns it into `peer:<contact>/<export>` rows.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a2a_types::AgentCard;
use agent::communication::network::{
    ChatPaused, ContactOffer, ContactOffers, OFFERS_TTL, OfferKind, OfferState,
};
use http_body_util::BodyExt as _;
use hyper_util::rt::TokioIo;

use crate::Ctx;
use crate::config::{FirstContact, ShareMode, Staged};

/// The per-reader card a contact fetches over `theuth/a2a/1`.
pub const CARD_PATH: &str = crate::config::CARD_SUFFIX;
/// The best-effort notice a sharer posts when what the reader may reach changed.
pub const NOTICE_PATH: &str = "/theuth/offers-changed";
/// Most offers kept from one card.
pub const OFFERS_MAX: usize = 64;
/// Longest offer title kept (the card's text limit).
pub const TITLE_MAX: usize = super::CARD_TEXT_MAX;
/// Longest refresh or card error kept for a log line.
const ERROR_MAX: usize = 200;
/// One fetch or notice's deadline.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Card bytes kept for the members/v1 extension's wrapper and the JWS (191 D1).
const CARD_MARGIN: usize = 4 * 1024;
/// Pairing: both sides pin on their own Accept, so the first fetch may beat the other's pin.
const PAIRED_RETRIES: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(10),
];

/// One contact's last fetched offers.
#[derive(Default)]
pub(crate) struct Cached {
    pub offers: Vec<ContactOffer>,
    /// 186 (D10): the last card advertised [`super::EXT_FULL`].
    pub full: bool,
    /// 191 (D8): the `machine` of the last card that verified (`""` until one did).
    pub machine: String,
    /// The last refresh, whatever its outcome (the TTL counts from it).
    pub at: Option<Instant>,
    /// A card of it verified this run: `offers` is what it shares, not a placeholder. Only
    /// such lists reach the agent, which reads a contact missing from them as unknown.
    pub fetched: bool,
    /// The last refresh failure logged (each distinct one once).
    pub error: Option<String>,
    /// A noticed refresh is running; `again` is one more asked for meanwhile.
    pub(super) busy: bool,
    pub(super) again: bool,
    /// Refreshes run (every outcome), for the coalescing test.
    #[cfg(test)]
    pub(super) refreshes: usize,
}

/// [`Ctx::offers_noticed`]'s claim on one contact's `busy` flag: released on drop unless
/// the refresh loop already released it under the lock (`done`).
pub(super) struct Busy {
    pub(super) ctx: Arc<Ctx>,
    pub(super) eid: String,
    pub(super) done: bool,
}

impl Drop for Busy {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if let Some(c) = self.ctx.offers.lock().get_mut(&self.eid) {
            c.busy = false;
            c.again = false;
        }
    }
}

/// Remote error text cut for a log line.
pub(crate) fn clip(s: &str) -> String {
    s.chars().take(ERROR_MAX).collect()
}

/// The exports `staged` grants the contact `alias`, as its card lists them. A chat's pause
/// (archived, messaging off) is the caller's to add: it needs the agent.
pub(crate) fn granted(staged: &Staged, alias: &str) -> Vec<ContactOffer> {
    let Some(peer) = staged.peers.get(alias) else {
        return Vec::new();
    };
    let facts = staged.contacts.get(alias);
    peer.exports
        .iter()
        .filter_map(|name| {
            let (kind, description) = match (staged.exports.get(name), staged.agents.get(name)) {
                (Some(e), _) => (OfferKind::Chat, e.description.trim()),
                (None, Some(a)) => (OfferKind::Agent, a.description.trim()),
                (None, None) => return None,
            };
            let state = match facts.and_then(|f| f.first_contact.get(name)) {
                Some(FirstContact::Allow) => OfferState::Allowed,
                Some(FirstContact::Block) => OfferState::Unavailable,
                _ => OfferState::AskFirst,
            };
            Some(ContactOffer {
                name: name.clone(),
                title: if description.is_empty() {
                    name.clone()
                } else {
                    description.to_owned()
                },
                kind,
                state,
                // 190 (D1): every share is full; the card keeps its `full` tag (D9).
                mode: ShareMode::Full,
            })
        })
        .collect()
}

/// The offers a verified card lists (untrusted text: names checked and unique, titles cut to
/// [`TITLE_MAX`] without control characters).
pub(crate) fn of_card(card: &AgentCard) -> Vec<ContactOffer> {
    let mut seen = std::collections::HashSet::new();
    card.skills
        .iter()
        .filter(|s| crate::config::check_alias(&s.id).is_ok() && seen.insert(s.id.as_str()))
        .take(OFFERS_MAX)
        .map(|s| {
            let tagged = |t: &str| s.tags.iter().any(|x| x == t);
            let title: String = s
                .description
                .chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
                .trim()
                .chars()
                .take(TITLE_MAX)
                .collect();
            ContactOffer {
                name: s.id.clone(),
                title: if title.is_empty() {
                    s.id.clone()
                } else {
                    title
                },
                kind: if tagged(OfferKind::Agent.as_str()) {
                    OfferKind::Agent
                } else {
                    OfferKind::Chat
                },
                state: if tagged(OfferState::Allowed.as_str()) {
                    OfferState::Allowed
                } else if tagged(OfferState::Unavailable.as_str()) {
                    OfferState::Unavailable
                } else {
                    OfferState::AskFirst
                },
                // 186: an older card has no mode tag: cautious.
                mode: if tagged(ShareMode::Full.as_str()) {
                    ShareMode::Full
                } else {
                    ShareMode::Cautious
                },
            }
        })
        .collect()
}

/// 191 (D1): the members/v1 payload of a verified card, if it carries one (unverified copies).
pub(crate) fn card_members(card: &AgentCard) -> Option<super::rosters::Carried> {
    let params = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == super::rosters::EXT_MEMBERS)?
        .params
        .clone()?;
    serde_json::from_value(serde_json::Value::Object(params.into_iter().collect())).ok()
}

/// One request to the contact `eid` on a fresh `theuth/a2a/1` stream: its status and body
/// (at most [`super::CARD_BYTES_MAX`]).
async fn call(
    net: &mesh::net::Net,
    eid: &str,
    method: hyper::Method,
    path: &str,
) -> Result<(hyper::StatusCode, bytes::Bytes), String> {
    call_with(net, eid, method, path, None).await
}

/// [`call`] with a JSON body (191: a roster push).
pub(crate) async fn call_with(
    net: &mesh::net::Net,
    eid: &str,
    method: hyper::Method,
    path: &str,
    body: Option<bytes::Bytes>,
) -> Result<(hyper::StatusCode, bytes::Bytes), String> {
    let go = async {
        let stream = net
            .a2a_stream(eid, &[], None)
            .await
            .map_err(|e| format!("contact unreachable: {e:#}"))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let mut req = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, "theuth");
        if body.is_some() {
            req = req.header(hyper::header::CONTENT_TYPE, "application/json");
        }
        let req = req
            .body(http_body_util::Full::new(body.unwrap_or_default()))
            .map_err(|e| e.to_string())?;
        let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body = http_body_util::Limited::new(resp.into_body(), super::CARD_BYTES_MAX)
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .to_bytes();
        Ok((status, body))
    };
    tokio::time::timeout(CALL_TIMEOUT, go)
        .await
        .map_err(|_| "no answer from the contact in time".to_owned())?
}

impl Ctx {
    /// The card served to the contact pinned to `eid`: this engine's public card plus, as
    /// skills, the exports granted to that contact (a paused chat reads `unavailable`). 191
    /// (D1): plus the members/v1 extension with the caller's own chains, when one exists (the
    /// card's JWS covers it).
    pub(crate) async fn contact_card(&self, eid: &str) -> Result<AgentCard, String> {
        let live = self.shared.read().clone();
        let alias = live
            .staged
            .contact_by_eid(eid)
            .ok_or("not a contact")?
            .to_owned();
        let mut offers = granted(&live.staged, &alias);
        if let Ok(agents) = self.agents() {
            for o in offers.iter_mut().filter(|o| o.kind == OfferKind::Chat) {
                let Some(e) = live.staged.exports.get(&o.name) else {
                    continue;
                };
                if !matches!(
                    self.ask(&agents, ChatPaused { session: e.session }).await,
                    Ok(None)
                ) {
                    o.state = OfferState::Unavailable;
                }
            }
        }
        let net = self.net().ok_or("this engine has no mesh endpoint")?;
        let mut facts = self.card_facts(&net);
        facts.skills = offers;
        let mut card = super::card(&facts);
        let mut carried = self.carried_to(eid).await?;
        // Within the card cap a reader enforces: the card's own bytes, a margin for the
        // extension's wrapper and the JWS, and the rest for the chains ([`rosters::trim`]).
        let own = serde_json::to_vec(&card).map_err(|e| e.to_string())?.len();
        super::rosters::trim(
            &mut carried,
            super::CARD_BYTES_MAX.saturating_sub(own + CARD_MARGIN),
        );
        if !carried.chains.is_empty() {
            let params = match serde_json::to_value(&carried) {
                Ok(serde_json::Value::Object(m)) => m.into_iter().collect(),
                _ => return Err("members payload".into()),
            };
            card.capabilities
                .extensions
                .get_or_insert_with(Vec::new)
                .push(a2a_types::AgentExtension {
                    uri: super::rosters::EXT_MEMBERS.to_owned(),
                    description: None,
                    required: Some(false),
                    params: Some(params),
                });
        }
        super::sign_card(card, &facts.endpoint, &|m: &[u8]| net.sign(m))
    }

    /// A contact said what it shares with us changed (`NOTICE_PATH`): fetch its card again.
    /// At most one noticed refresh runs per contact; notices meanwhile collapse into one
    /// more refresh after it (review 1). However the refresh task ends (a panic included),
    /// `busy` is cleared, so a later notice runs again (review 2).
    pub(crate) fn offers_noticed(self: &Arc<Self>, eid: &str) {
        {
            let mut all = self.offers.lock();
            let c = all.entry(eid.to_owned()).or_default();
            if c.busy {
                c.again = true;
                return;
            }
            c.busy = true;
        }
        let busy = Busy {
            ctx: self.clone(),
            eid: eid.to_owned(),
            done: false,
        };
        tokio::spawn(async move {
            let mut busy = busy;
            loop {
                busy.ctx.refresh_offers(&busy.eid).await;
                let mut all = busy.ctx.offers.lock();
                let Some(e) = all.get_mut(&busy.eid) else {
                    return;
                };
                if !std::mem::take(&mut e.again) {
                    // Cleared under the same lock a notice checks it under: none is lost.
                    e.busy = false;
                    drop(all);
                    busy.done = true;
                    return;
                }
            }
        });
    }

    /// Fetch the contact `eid`'s card, verify it against the pin and keep its offers; a
    /// failure keeps the last list and is logged once (cut to 200 characters). `true` when
    /// it took; the agent and `A2aChanged` hear of it only when the list changed.
    pub(crate) async fn refresh_offers(&self, eid: &str) -> bool {
        let mut members = None;
        let outcome = match self.net() {
            Some(net) => match call(&net, eid, hyper::Method::GET, CARD_PATH).await {
                Ok((s, body)) if s.is_success() => serde_json::from_slice::<AgentCard>(&body)
                    .map_err(|e| format!("card: {e}"))
                    .and_then(|card| {
                        super::verify_card(&card, eid)?;
                        members = card_members(&card);
                        Ok((
                            of_card(&card),
                            super::card_full(&card),
                            super::card_machine(&card).0,
                        ))
                    }),
                Ok((s, _)) => Err(format!("card: HTTP {s}")),
                Err(e) => Err(e),
            },
            None => Err("this engine has no mesh endpoint".to_owned()),
        };
        // 191 (D1): the card carries the contact's chains; each copy is verified on its own
        // (a card without the extension changes nothing).
        if let Some(m) = &members {
            self.ingest(m, eid).await;
        }
        let took = outcome.is_ok();
        let changed = {
            let mut all = self.offers.lock();
            let c = all.entry(eid.to_owned()).or_default();
            c.at = Some(Instant::now());
            #[cfg(test)]
            {
                c.refreshes += 1;
            }
            match outcome {
                Ok((offers, full, machine)) => {
                    c.error = None;
                    let first = !std::mem::replace(&mut c.fetched, true);
                    let was = std::mem::replace(&mut c.full, full);
                    let named = std::mem::replace(&mut c.machine, machine) != c.machine;
                    (std::mem::replace(&mut c.offers, offers) != c.offers)
                        || was != full
                        || named
                        || first
                }
                Err(e) => {
                    let e = clip(&e);
                    if c.error.as_ref() != Some(&e) {
                        tracing::info!(%eid, "a2a: contact's shared list not refreshed (kept the last one): {e}");
                        c.error = Some(e);
                    }
                    false
                }
            }
        };
        if changed {
            self.push_offers().await;
            self.rt.publish(crate::A2aChanged {}).await;
        }
        took
    }

    /// Pairing (and boot): fetch the contact's card, retrying while its side finishes
    /// pinning us.
    pub(crate) fn refresh_offers_soon(self: &Arc<Self>, eid: &str) {
        let (c, eid) = (self.clone(), eid.to_owned());
        tokio::spawn(async move {
            if c.refresh_offers(&eid).await {
                return;
            }
            for wait in PAIRED_RETRIES {
                tokio::time::sleep(wait).await;
                if c.refresh_offers(&eid).await {
                    return;
                }
            }
        });
    }

    /// `discover_peers` found the offers stale, or a send found a contact's unknown: refetch,
    /// together, the card of every machine of every contact (191: every member it admits,
    /// not one) older than [`OFFERS_TTL`] or never verified this run.
    pub(crate) async fn refresh_stale_offers(&self) {
        let live = self.shared.read().clone();
        let stale: Vec<String> = {
            let all = self.offers.lock();
            live.staged
                .contacts
                .values()
                .filter(|f| !f.blocked)
                .flat_map(|f| f.eids.iter())
                .filter(|eid| {
                    all.get(*eid).is_none_or(|c| {
                        !c.fetched || c.at.is_none_or(|at| at.elapsed() >= OFFERS_TTL)
                    })
                })
                .cloned()
                .collect()
        };
        futures::future::join_all(stale.iter().map(|eid| self.refresh_offers(eid))).await;
    }

    /// 191: the machine of the contact admitting `eid` whose last card counts: the most
    /// recent one that took, else `eid`.
    pub(crate) fn offers_eid(&self, eid: &str) -> String {
        let live = self.shared.read().clone();
        let Some(f) = live
            .staged
            .contact_by_eid(eid)
            .and_then(|a| live.staged.contacts.get(a))
        else {
            return eid.to_owned();
        };
        let all = self.offers.lock();
        f.eids
            .iter()
            .filter_map(|e| {
                let c = all.get(e)?;
                (c.error.is_none()).then_some((c.at?, e))
            })
            .max()
            .map_or_else(|| eid.to_owned(), |(_, e)| e.clone())
    }

    /// The offers last fetched from the contact pinned to `eid`.
    pub(crate) fn offers_of(&self, eid: &str) -> Vec<ContactOffer> {
        self.offers
            .lock()
            .get(eid)
            .map(|c| c.offers.clone())
            .unwrap_or_default()
    }

    /// 186 (D10): the contact pinned to `eid` last served a card that collaborates in full.
    pub(crate) fn full_of(&self, eid: &str) -> bool {
        self.offers.lock().get(eid).is_some_and(|c| c.full)
    }

    /// 191 (D8): the machine of `eid`'s last verified card, else the first 8 characters of
    /// `eid` (in memory: after a restart until its card is fetched again).
    pub(crate) fn machine_name(&self, eid: &str) -> String {
        self.offers
            .lock()
            .get(eid)
            .map(|c| c.machine.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| eid.get(..8).unwrap_or(eid).to_owned())
    }

    /// Hand the agent every live contact's offers, by today's alias.
    pub(crate) async fn push_offers(&self) {
        let Ok(agents) = self.agents() else { return };
        let live = self.shared.read().clone();
        let from: Vec<(String, String)> = live
            .staged
            .contacts
            .iter()
            .filter(|(_, f)| !f.blocked)
            .map(|(alias, f)| (alias.clone(), self.offers_eid(&f.eid)))
            .collect();
        let map: BTreeMap<String, Vec<ContactOffer>> = {
            let all = self.offers.lock();
            from.into_iter()
                .filter_map(|(alias, eid)| {
                    Some((alias, all.get(&eid).filter(|c| c.fetched)?.offers.clone()))
                })
                .collect()
        };
        let _ = agents.tell(ContactOffers(map)).await;
    }

    /// 183 F1: tell each contact pinned to one of `eids` that what it may reach here
    /// changed, so it fetches our card again. Best effort: a contact that misses it catches
    /// up within the TTL.
    pub(crate) fn notify_offers(&self, eids: Vec<String>) {
        let Some(net) = self.net() else { return };
        for eid in eids {
            let net = net.clone();
            tokio::spawn(async move {
                if let Err(e) = call(&net, &eid, hyper::Method::POST, NOTICE_PATH).await {
                    tracing::debug!(%eid, "a2a: offers notice not delivered: {e}");
                }
            });
        }
    }
}

/// The contacts (191: every machine of each, by endpoint id) whose granted offers differ
/// between `prev` and `next`; a contact new in `next` is left out (it fetches our card at
/// pairing).
pub(crate) fn changed(prev: &Staged, next: &Staged) -> Vec<String> {
    let before: HashMap<&str, (&str, Vec<ContactOffer>)> = prev
        .contacts
        .iter()
        .map(|(alias, f)| (f.eid.as_str(), (alias.as_str(), granted(prev, alias))))
        .collect();
    next.contacts
        .iter()
        .filter(|(_, f)| !f.blocked)
        .filter_map(|(alias, f)| {
            let (_, old) = before.get(f.eid.as_str())?;
            (*old != granted(next, alias)).then(|| f.eids.clone())
        })
        .flatten()
        .collect()
}
