//! Live public shares in Slack (design doc 210 §7.4 item 9, D12, G4): one
//! status message in the owner's DM while anything is public, edited in
//! place on every `share.changed` and to [`STOPPED_TEXT`] at the end, and
//! the `/theuth share [list|stop [id|all]]` argument grammar.
//!
//! Everything here is pure: [`share_status_text`] renders, [`LiveMessage`]
//! decides post / update / end; `channel.rs` does the Slack calls.
//! TODO(210-M4): pin the status message (`pins.add`) once `api.rs` and the
//! manifest's scopes carry it; today it is edited in place only.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Display;

use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::{Value, json};
use share::model::{Share, ShareState};

use crate::ui::clip;

/// What the status message becomes once nothing is public.
pub const STOPPED_TEXT: &str = "Public sharing stopped";
/// What it becomes when another engine took the hub lease (and posts its own).
pub const MOVED_TEXT: &str = "Public sharing status moved to the hub machine";
/// Section `text` cap (Block Kit).
const SECTION_MAX: usize = 3000;
/// Button label cap (Block Kit `plain_text` in a button).
const LABEL_MAX: usize = 75;
/// Per-share Stop buttons beside Stop all.
const STOP_BUTTONS: usize = 4;

/// Counts as public for the indicators (UI1): live, offline or pending.
#[must_use]
pub fn is_active(s: &Share) -> bool {
    matches!(
        s.state,
        ShareState::Live | ShareState::Offline | ShareState::Pending
    )
}

/// Now, Unix ms.
#[must_use]
pub fn now_ms() -> u64 {
    u64::try_from(Utc::now().timestamp_millis()).unwrap_or(0)
}

/// The live message for `shares` at `now_ms` in local time: a header, one
/// line per active share with its URL and expiry, and the Stop-all hint.
/// `None` when none is active.
#[must_use]
pub fn share_status_text(shares: &[Share], now_ms: u64) -> Option<String> {
    status_text_in(shares, now_ms, &Local)
}

/// `/theuth share`: the active shares with their ids, or that none is.
#[must_use]
pub fn share_list_text(shares: &[Share], now_ms: u64) -> String {
    list_text_in(shares, now_ms, &Local)
}

fn status_text_in<Tz: TimeZone>(shares: &[Share], now_ms: u64, tz: &Tz) -> Option<String>
where
    Tz::Offset: Display,
{
    let active = sorted_active(shares);
    if active.is_empty() {
        return None;
    }
    let mut out = format!("● Sharing publicly · {} live", active.len());
    for s in &active {
        out.push_str("\n• ");
        out.push_str(&line(s, now_ms, tz));
    }
    out.push_str("\n`/theuth share stop` stops all");
    Some(out)
}

fn list_text_in<Tz: TimeZone>(shares: &[Share], now_ms: u64, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let active = sorted_active(shares);
    if active.is_empty() {
        return "no public shares".into();
    }
    let mut out = format!("● Sharing publicly · {} live", active.len());
    for s in &active {
        out.push_str(&format!("\n• {} · `{}`", line(s, now_ms, tz), s.id));
    }
    out.push_str("\n`/theuth share stop [id|all]` stops one or all");
    out
}

/// Active shares, oldest first.
fn sorted_active(shares: &[Share]) -> Vec<&Share> {
    let mut active: Vec<&Share> = shares.iter().filter(|s| is_active(s)).collect();
    active.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
    active
}

/// `https://x.tun.zip/ — expires 14:02 (in 3h)`, `… — pending`, `… — offline · expires …`.
fn line<Tz: TimeZone>(s: &Share, now_ms: u64, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let expiry = || match s.expires_ms {
        Some(at) => format!("expires {}", expires(at, now_ms, tz)),
        None => "no expiry".into(),
    };
    match s.state {
        ShareState::Pending => format!("{} — pending", s.url),
        ShareState::Offline => format!("{} — offline · {}", s.url, expiry()),
        _ => format!("{} — {}", s.url, expiry()),
    }
}

/// `14:02 (in 3h)` today, `Tue 24 Nov 09:00 (in 2d)` later.
fn expires<Tz: TimeZone>(at_ms: u64, now_ms: u64, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let local = |ms: u64| {
        DateTime::<Utc>::from_timestamp_millis(i64::try_from(ms).unwrap_or(i64::MAX))
            .map(|t| t.with_timezone(tz))
    };
    let clock = match (local(at_ms), local(now_ms)) {
        (Some(at), Some(now)) if at.date_naive() == now.date_naive() => {
            at.format("%H:%M").to_string()
        }
        (Some(at), _) => at.format("%a %-d %b %H:%M").to_string(),
        (None, _) => return "later".into(),
    };
    format!("{clock} ({})", left(at_ms.saturating_sub(now_ms)))
}

/// `in 45m`, `in 3h`, `in 2d`; `now` at or past the end.
fn left(ms: u64) -> String {
    let m = ms.div_ceil(60_000);
    match m {
        0 => "now".into(),
        1..60 => format!("in {m}m"),
        60..2880 => format!("in {}h", m / 60),
        _ => format!("in {}d", m / 1440),
    }
}

/// The live message's Block Kit: the text, then Stop buttons (one per
/// share, at most [`STOP_BUTTONS`]) and Stop all. Clicks run
/// `/theuth share stop [id]` through the owner gate.
#[must_use]
pub fn status_blocks(text: &str, shares: &[Share]) -> Value {
    let mut buttons: Vec<Value> = sorted_active(shares)
        .into_iter()
        .take(STOP_BUTTONS)
        .map(|s| {
            let label = format!("Stop {}{}", s.host, s.path.trim_end_matches('/'));
            json!({
                "type": "button",
                "action_id": format!("cmd:share:stop:{}", s.id),
                "text": { "type": "plain_text", "text": clip(&label, LABEL_MAX) },
                "value": s.id,
            })
        })
        .collect();
    buttons.push(json!({
        "type": "button",
        "action_id": "cmd:share:stop",
        "style": "danger",
        "text": { "type": "plain_text", "text": "Stop all" },
    }));
    json!([
        { "type": "section", "text": { "type": "mrkdwn", "text": clip(text, SECTION_MAX) } },
        { "type": "actions", "elements": buttons },
    ])
}

/// The final form: text only, no buttons.
#[must_use]
pub fn ended_blocks(text: &str) -> Value {
    json!([{ "type": "section", "text": { "type": "mrkdwn", "text": text } }])
}

/// What to do with the status message.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveOp {
    /// Post a new message (then [`LiveMessage::posted`] with its ts).
    Post {
        /// Fallback text.
        text: String,
        /// Block Kit array.
        blocks: Value,
    },
    /// `chat.update` the message at `ts`.
    Update {
        /// The message.
        ts: String,
        /// Block Kit array.
        blocks: Value,
    },
    /// Edit the message at `ts` to its final form; it is forgotten.
    End {
        /// The message.
        ts: String,
        /// [`STOPPED_TEXT`] or [`MOVED_TEXT`].
        text: String,
    },
}

/// The active shares as last seen and the status message showing them.
#[derive(Debug, Default)]
pub struct LiveMessage {
    /// Active shares by id.
    shares: BTreeMap<String, Share>,
    /// Ids seen stopped: `Stopped` is terminal, so an older snapshot never revives them.
    ended: HashSet<String>,
    /// The posted message.
    ts: Option<String>,
    /// The text it shows.
    shown: Option<String>,
}

impl LiveMessage {
    /// A `share.changed` record (or a seeded row).
    pub fn observe(&mut self, share: Share) {
        if self.ended.contains(&share.id) {
            return;
        }
        if is_active(&share) {
            self.shares.insert(share.id.clone(), share);
        } else {
            if share.state == ShareState::Stopped {
                self.ended.insert(share.id.clone());
            }
            self.shares.remove(&share.id);
        }
    }

    /// The active shares, by id.
    #[must_use]
    pub fn active(&self) -> Vec<Share> {
        self.shares.values().cloned().collect()
    }

    /// The next Slack call, if any. `holding` = this engine holds the hub
    /// lease: every engine sees every share, only the holder shows them.
    pub fn next(&mut self, now_ms: u64, holding: bool) -> Option<LiveOp> {
        let shares = self.active();
        let text = if holding {
            share_status_text(&shares, now_ms)
        } else {
            None
        };
        match (text, self.ts.clone()) {
            (Some(text), None) => Some(LiveOp::Post {
                blocks: status_blocks(&text, &shares),
                text,
            }),
            (Some(text), Some(ts)) if self.shown.as_deref() != Some(text.as_str()) => {
                let blocks = status_blocks(&text, &shares);
                self.shown = Some(text);
                Some(LiveOp::Update { ts, blocks })
            }
            (None, Some(ts)) => {
                self.ts = None;
                self.shown = None;
                let text = if shares.is_empty() {
                    STOPPED_TEXT
                } else {
                    MOVED_TEXT
                };
                Some(LiveOp::End {
                    ts,
                    text: text.into(),
                })
            }
            _ => None,
        }
    }

    /// A [`LiveOp::Post`] landed at `ts` showing `text`.
    pub fn posted(&mut self, ts: String, text: String) {
        self.ts = Some(ts);
        self.shown = Some(text);
    }
}

/// `/theuth share …` arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareCmd {
    /// Bare or `list`.
    List,
    /// `stop` or `stop all`.
    StopAll,
    /// `stop <id|url|host>`.
    Stop(String),
    /// Anything else.
    Usage,
}

/// The usage line.
pub const USAGE: &str = "usage: /theuth share [list | stop [<id>|all]]";

/// Parses the words after `share`.
#[must_use]
pub fn parse(args: &[String]) -> ShareCmd {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["list"] => ShareCmd::List,
        ["stop"] | ["stop", "all"] => ShareCmd::StopAll,
        ["stop", id] => ShareCmd::Stop((*id).to_owned()),
        _ => ShareCmd::Usage,
    }
}

/// The one active share `key` names: its full id, else a unique id prefix
/// or suffix, its host or its URL. `Err` is reply text.
pub fn pick<'a>(shares: &'a [Share], key: &str) -> Result<&'a Share, String> {
    let key = key.trim_matches(['<', '>']);
    if let Some(s) = shares.iter().find(|s| s.id == key) {
        return Ok(s);
    }
    let bare = key.trim_end_matches('/');
    let hits: Vec<&Share> = shares
        .iter()
        .filter(|s| is_active(s))
        .filter(|s| {
            s.id.starts_with(key)
                || s.id.ends_with(key)
                || s.host == key
                || s.url.trim_end_matches('/') == bare
        })
        .collect();
    match hits.as_slice() {
        [s] => Ok(s),
        [] => Err(format!("no live share {key:?}")),
        many => Err(format!(
            "{key:?} matches {} shares; use the full id",
            many.len()
        )),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use share::model::{Access, ApprovedBy, CreatedBy, Source};

    use super::*;

    /// 2026-11-23 12:00:00 UTC.
    const NOW: u64 = 1_795_435_200_000;
    const HOUR: u64 = 3_600_000;

    pub(crate) fn share(id: &str, host: &str, state: ShareState, expires_ms: Option<u64>) -> Share {
        Share {
            id: id.into(),
            engine: "engine-a".into(),
            edge: "edge".into(),
            domain: "tun.zip".into(),
            host: host.into(),
            path: "/".into(),
            url: format!("https://{host}/"),
            source: Source::Port(3000),
            access: Access::Public,
            interstitial: false,
            created_ms: NOW - HOUR,
            live_ms: Some(NOW - HOUR),
            expires_ms,
            created_by: CreatedBy::Settings("settings".into()),
            approved_by: ApprovedBy {
                approval_id: "a1".into(),
                surface: "slack".into(),
            },
            state,
            stopped_ms: None,
            stopped_by: None,
        }
    }

    #[test]
    fn status_text_for_none_one_and_two() {
        assert_eq!(status_text_in(&[], NOW, &Utc), None);
        let stopped = share("s0", "old.sky.tun.zip", ShareState::Stopped, None);
        assert_eq!(
            status_text_in(std::slice::from_ref(&stopped), NOW, &Utc),
            None
        );

        let a = share(
            "s1",
            "app.sky.tun.zip",
            ShareState::Live,
            Some(NOW + 2 * HOUR + 2 * 60_000),
        );
        assert_eq!(
            status_text_in(&[a.clone(), stopped], NOW, &Utc).as_deref(),
            Some(
                "● Sharing publicly · 1 live\n\
                 • https://app.sky.tun.zip/ — expires 14:02 (in 2h)\n\
                 `/theuth share stop` stops all"
            )
        );

        let mut b = share("s2", "docs.sky.tun.zip", ShareState::Pending, None);
        b.created_ms = NOW;
        let mut c = share(
            "s3",
            "x.sky.tun.zip",
            ShareState::Offline,
            Some(NOW + 2 * 86_400_000),
        );
        c.created_ms = NOW + 1;
        assert_eq!(
            status_text_in(&[c, b, a], NOW, &Utc).as_deref(),
            Some(
                "● Sharing publicly · 3 live\n\
                 • https://app.sky.tun.zip/ — expires 14:02 (in 2h)\n\
                 • https://docs.sky.tun.zip/ — pending\n\
                 • https://x.sky.tun.zip/ — offline · expires Wed 25 Nov 12:00 (in 2d)\n\
                 `/theuth share stop` stops all"
            )
        );
    }

    #[test]
    fn relative_expiry() {
        assert_eq!(left(0), "now");
        assert_eq!(left(1), "in 1m");
        assert_eq!(left(45 * 60_000), "in 45m");
        assert_eq!(left(3 * HOUR), "in 3h");
        assert_eq!(left(47 * HOUR), "in 47h");
        assert_eq!(left(72 * HOUR), "in 3d");
    }

    #[test]
    fn list_text_carries_ids() {
        let a = share("s1", "app.sky.tun.zip", ShareState::Live, None);
        assert_eq!(list_text_in(&[], NOW, &Utc), "no public shares");
        assert_eq!(
            list_text_in(&[a], NOW, &Utc),
            "● Sharing publicly · 1 live\n\
             • https://app.sky.tun.zip/ — no expiry · `s1`\n\
             `/theuth share stop [id|all]` stops one or all"
        );
    }

    #[test]
    fn live_message_posts_updates_and_ends() {
        let mut m = LiveMessage::default();
        assert_eq!(m.next(NOW, true), None);

        let a = share("s1", "app.sky.tun.zip", ShareState::Live, None);
        m.observe(a.clone());
        let Some(LiveOp::Post { text, blocks }) = m.next(NOW, true) else {
            panic!("expected a post");
        };
        assert!(text.contains("· 1 live"), "{text}");
        let ids: Vec<&str> = blocks[1]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["action_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["cmd:share:stop:s1", "cmd:share:stop"]);
        m.posted("1.1".into(), text);
        // Same text: nothing to do.
        m.observe(a.clone());
        assert_eq!(m.next(NOW, true), None);

        m.observe(share("s2", "b.sky.tun.zip", ShareState::Pending, None));
        let Some(LiveOp::Update { ts, blocks }) = m.next(NOW, true) else {
            panic!("expected an update");
        };
        assert_eq!(ts, "1.1");
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("· 2 live"), "{text}");

        m.observe(share("s2", "b.sky.tun.zip", ShareState::Stopped, None));
        let mut stopped = a;
        stopped.state = ShareState::Stopped;
        m.observe(stopped);
        assert_eq!(
            m.next(NOW, true),
            Some(LiveOp::End {
                ts: "1.1".into(),
                text: STOPPED_TEXT.into(),
            })
        );
        assert_eq!(m.next(NOW, true), None);
        // An older snapshot never revives a stopped share.
        m.observe(share("s2", "b.sky.tun.zip", ShareState::Live, None));
        assert_eq!(m.next(NOW, true), None);
    }

    #[test]
    fn live_message_leaves_with_the_lease() {
        let mut m = LiveMessage::default();
        m.observe(share("s1", "app.sky.tun.zip", ShareState::Live, None));
        assert_eq!(m.next(NOW, false), None);
        let Some(LiveOp::Post { text, .. }) = m.next(NOW, true) else {
            panic!("expected a post");
        };
        m.posted("1.1".into(), text);
        assert_eq!(
            m.next(NOW, false),
            Some(LiveOp::End {
                ts: "1.1".into(),
                text: MOVED_TEXT.into(),
            })
        );
    }

    #[test]
    fn parses_share_args() {
        let w = |s: &str| -> Vec<String> { s.split_whitespace().map(str::to_owned).collect() };
        assert_eq!(parse(&w("")), ShareCmd::List);
        assert_eq!(parse(&w("list")), ShareCmd::List);
        assert_eq!(parse(&w("stop")), ShareCmd::StopAll);
        assert_eq!(parse(&w("stop all")), ShareCmd::StopAll);
        assert_eq!(parse(&w("stop abc")), ShareCmd::Stop("abc".into()));
        assert_eq!(parse(&w("stop a b")), ShareCmd::Usage);
        assert_eq!(parse(&w("start")), ShareCmd::Usage);
    }

    #[test]
    fn picks_by_id_prefix_host_or_url() {
        let rows = [
            share("0191-aaaa", "app.sky.tun.zip", ShareState::Live, None),
            share("0191-bbbb", "docs.sky.tun.zip", ShareState::Live, None),
        ];
        assert_eq!(pick(&rows, "0191-aaaa").unwrap().id, "0191-aaaa");
        assert_eq!(pick(&rows, "bbbb").unwrap().id, "0191-bbbb");
        assert_eq!(pick(&rows, "app.sky.tun.zip").unwrap().id, "0191-aaaa");
        assert_eq!(
            pick(&rows, "<https://docs.sky.tun.zip>").unwrap().id,
            "0191-bbbb"
        );
        assert!(pick(&rows, "0191").unwrap_err().contains("matches 2"));
        assert!(pick(&rows, "zzz").unwrap_err().contains("no live share"));
    }
}
