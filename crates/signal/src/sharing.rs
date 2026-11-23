//! Live public shares on Signal (design doc 210 §7.4 item 9, D12, G4): the owner-only
//! `/share`, `/share list`, `/share stop [id|all]` (Stop is never gated by an approval) and a
//! notice to the owner on the transitions: the first active share sends the list, the last
//! one ending sends [`STOPPED_TEXT`]. Signal cannot pin, and its edits are unreliable on
//! linked devices, so there is no single message kept in place as on Telegram.
//!
//! TODO(210-M4): the text and the `/share` arms mirror `crates/telegram/src/sharing.rs`;
//! move both into `chat-common` once a slice owns it.

use std::collections::BTreeMap;
use std::fmt::Display;

use chrono::{Local, TimeZone};
use share::model::{Share, ShareState, StoppedBy};

/// The notice when nothing is shared any more.
pub const STOPPED_TEXT: &str = "Public sharing stopped";
/// The `/share` reply when this engine runs no share plugin.
pub const NOT_RUNNING: &str = "sharing is not running on this engine";
const USAGE: &str = "usage: /share [list | stop [<id>|all]]";

/// Counts as live for the indicators (UI1): Live, Offline or Pending.
#[must_use]
pub fn active(s: &Share) -> bool {
    matches!(
        s.state,
        ShareState::Live | ShareState::Offline | ShareState::Pending
    )
}

/// The list for `shares` at `now_ms` (local clock), `None` when none is active.
#[must_use]
pub fn share_status_text(shares: &[Share], now_ms: u64) -> Option<String> {
    render(shares.iter(), now_ms, &Local)
}

/// `● Sharing publicly · N live`, one line per active share (oldest first) with its id, the
/// stop hint.
fn render<'a, Tz>(shares: impl Iterator<Item = &'a Share>, now_ms: u64, tz: &Tz) -> Option<String>
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    let mut live: Vec<&Share> = shares.filter(|s| active(s)).collect();
    if live.is_empty() {
        return None;
    }
    live.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
    let mut out = format!("● Sharing publicly · {} live", live.len());
    for s in &live {
        let state = match s.state {
            ShareState::Pending => "pending · ",
            ShareState::Offline => "offline · ",
            ShareState::Live | ShareState::Stopped => "",
        };
        let expiry = s
            .expires_ms
            .map_or_else(|| "no expiry".to_owned(), |e| expires(e, now_ms, tz));
        out.push_str(&format!("\n{} — {state}{expiry} · {}", s.url, s.id));
    }
    out.push_str("\n\n/share stop stops everything · /share stop <id> stops one");
    Some(out)
}

/// `expires 14:02 (in 3h)`; another day shows the date.
fn expires<Tz>(at_ms: u64, now_ms: u64, tz: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    let local = |ms: u64| {
        i64::try_from(ms)
            .ok()
            .and_then(|ms| tz.timestamp_millis_opt(ms).single())
    };
    let clock = match (local(at_ms), local(now_ms)) {
        (Some(at), Some(now)) if at.date_naive() == now.date_naive() => {
            at.format("%H:%M").to_string()
        }
        (Some(at), _) => at.format("%b %-d %H:%M").to_string(),
        (None, _) => "later".to_owned(),
    };
    if at_ms <= now_ms {
        return format!("expires {clock} (due now)");
    }
    let m = (at_ms - now_ms) / 60_000;
    let left = if m < 1 {
        "<1m".to_owned()
    } else if m < 60 {
        format!("{m}m")
    } else if m < 48 * 60 {
        format!("{}h", (m + 30) / 60)
    } else {
        format!("{}d", (m + 12 * 60) / (24 * 60))
    };
    format!("expires {clock} (in {left})")
}

/// The owner's transition notices: the latest record of every share seen (the replicated
/// rows of other machines too) and whether "sharing" was last announced.
// ponytail: stopped records stay for the process's life so a late seed never revives one.
#[derive(Debug, Default)]
pub struct Notices {
    shares: BTreeMap<String, Share>,
    announced: bool,
}

impl Notices {
    /// A `share.changed` record: always the latest.
    pub fn apply(&mut self, share: Share) {
        self.shares.insert(share.id.clone(), share);
    }

    /// The boot list: never overrides a record an event already brought.
    pub fn seed(&mut self, list: Vec<Share>) {
        for s in list {
            self.shares.entry(s.id.clone()).or_insert(s);
        }
    }

    /// The notice to send now, if this is a transition.
    pub fn step(&mut self, now_ms: u64) -> Option<String> {
        self.step_in(now_ms, &Local)
    }

    fn step_in<Tz>(&mut self, now_ms: u64, tz: &Tz) -> Option<String>
    where
        Tz: TimeZone,
        Tz::Offset: Display,
    {
        match (self.announced, render(self.shares.values(), now_ms, tz)) {
            (false, Some(text)) => {
                self.announced = true;
                Some(text)
            }
            (true, None) => {
                self.announced = false;
                Some(STOPPED_TEXT.to_owned())
            }
            _ => None,
        }
    }

    /// The last notice did not go out: the next change sends it again.
    pub fn failed(&mut self) {
        self.announced = !self.announced;
    }
}

/// Unix ms now.
#[must_use]
pub fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// `/share [list | stop [<id>|all]]` (owner-only upstream); the reply text.
pub async fn command(arg: &str) -> anyhow::Result<String> {
    let (verb, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(v, r)| (v, r.trim()));
    if !matches!(verb, "" | "list" | "stop") || (verb != "stop" && !rest.is_empty()) {
        return Ok(USAGE.to_owned());
    }
    let Ok(engine) = share::rpc_support::engine() else {
        return Ok(NOT_RUNNING.to_owned());
    };
    if verb != "stop" {
        let list = engine.list().await?;
        return Ok(share_status_text(&list, now_ms())
            .unwrap_or_else(|| "Nothing is shared publicly.".to_owned()));
    }
    if rest.is_empty() || rest == "all" {
        let out = engine.stop_all().await?;
        return Ok(stop_all_text(out.stopped, out.edge_pending.as_deref()));
    }
    let list = engine.list().await?;
    match pick(&list, rest) {
        Ok(id) => {
            let s = engine.stop(&id, StoppedBy::User).await?;
            Ok(format!("Stopped {}", s.url))
        }
        Err(msg) => Ok(msg),
    }
}

/// The Stop all receipt.
#[must_use]
pub fn stop_all_text(stopped: usize, edge_pending: Option<&str>) -> String {
    let mut out = format!("Stopped all public sharing ({stopped} on this machine).");
    if let Some(why) = edge_pending {
        out.push_str(&format!(
            "\nThe sharing edge has not confirmed yet ({why}); it is told again when this machine reconnects."
        ));
    }
    out
}

/// The one active share `q` names: its id, a unique id prefix, its URL or host.
fn pick(list: &[Share], q: &str) -> Result<String, String> {
    let live = || list.iter().filter(|s| active(s));
    if let Some(s) = live().find(|s| s.id == q) {
        return Ok(s.id.clone());
    }
    let hits: Vec<&Share> = live()
        .filter(|s| {
            s.id.starts_with(q) || s.url == q || s.url.trim_end_matches('/') == q || s.host == q
        })
        .collect();
    match hits.as_slice() {
        [s] => Ok(s.id.clone()),
        [] => Err(format!("no live share matches {q}; /share list shows them")),
        _ => Err(format!(
            "{q} matches {} shares; use the full id",
            hits.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use share::model::{Access, ApprovedBy, CreatedBy, Source};

    use super::*;

    /// 2026-11-23 11:00 UTC.
    const NOW: u64 = 1_795_431_600_000;
    const HOUR: u64 = 3_600_000;

    fn share(id: &str, state: ShareState, expires_ms: Option<u64>) -> Share {
        Share {
            id: id.into(),
            engine: "me".into(),
            edge: "e".into(),
            domain: "tun.zip".into(),
            host: format!("{id}.tun.zip"),
            path: "/".into(),
            url: format!("https://{id}.tun.zip/"),
            source: Source::Port(5173),
            access: Access::Public,
            interstitial: true,
            created_ms: NOW - HOUR,
            live_ms: None,
            expires_ms,
            created_by: CreatedBy::Settings("settings".into()),
            approved_by: ApprovedBy {
                approval_id: "a".into(),
                surface: "human".into(),
            },
            state,
            stopped_ms: None,
            stopped_by: None,
        }
    }

    #[test]
    fn list_text_for_zero_one_and_two_shares() {
        assert_eq!(render([].iter(), NOW, &Utc), None);
        let x = share("x", ShareState::Live, Some(NOW + 3 * HOUR));
        assert_eq!(
            render([x.clone()].iter(), NOW, &Utc).unwrap(),
            "● Sharing publicly · 1 live\n\
             https://x.tun.zip/ — expires 14:00 (in 3h) · x\n\n\
             /share stop stops everything · /share stop <id> stops one"
        );
        let y = share("y", ShareState::Pending, Some(NOW + 50 * HOUR));
        let text = render([y, x].iter(), NOW, &Utc).unwrap();
        assert!(text.starts_with("● Sharing publicly · 2 live\n"), "{text}");
        assert!(
            text.contains("https://y.tun.zip/ — pending · expires Nov 25 13:00 (in 2d) · y"),
            "{text}"
        );
    }

    #[test]
    fn notices_fire_on_transitions_only() {
        let mut n = Notices::default();
        assert_eq!(n.step_in(NOW, &Utc), None);
        n.apply(share("x", ShareState::Pending, None));
        assert!(n.step_in(NOW, &Utc).unwrap().contains("1 live"));
        n.failed();
        assert!(n.step_in(NOW, &Utc).unwrap().contains("1 live"), "retried");
        n.apply(share("y", ShareState::Live, None));
        assert_eq!(n.step_in(NOW, &Utc), None, "no edit on Signal");
        n.apply(share("x", ShareState::Stopped, None));
        n.seed(vec![share("x", ShareState::Live, None)]);
        assert_eq!(n.step_in(NOW, &Utc), None);
        n.apply(share("y", ShareState::Stopped, None));
        assert_eq!(n.step_in(NOW, &Utc).as_deref(), Some(STOPPED_TEXT));
        assert_eq!(n.step_in(NOW, &Utc), None);
    }

    #[test]
    fn stop_targets_one_active_share() {
        let list = vec![
            share("0193aa01", ShareState::Live, None),
            share("0193aa02", ShareState::Offline, None),
            share("0193bb01", ShareState::Stopped, None),
        ];
        assert_eq!(pick(&list, "0193aa01").unwrap(), "0193aa01");
        assert_eq!(pick(&list, "0193aa02.tun.zip").unwrap(), "0193aa02");
        assert!(pick(&list, "0193aa").unwrap_err().contains("matches 2"));
        assert!(
            pick(&list, "0193bb01")
                .unwrap_err()
                .starts_with("no live share")
        );
        assert!(stop_all_text(0, None).ends_with("(0 on this machine)."));
    }

    #[tokio::test]
    async fn commands_without_a_running_engine_say_so() {
        for arg in ["", "list", "stop", "stop all", "stop 0193aa01"] {
            assert_eq!(command(arg).await.unwrap(), NOT_RUNNING, "{arg}");
        }
        assert_eq!(command("stopp").await.unwrap(), USAGE);
    }
}
