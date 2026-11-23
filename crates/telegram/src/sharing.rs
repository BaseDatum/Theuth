//! Live public shares on Telegram (design doc 210 §7.4 item 9, D12, G4): while anything is
//! shared publicly the owner's private chat holds ONE pinned message listing every live URL
//! with its expiry and a `Stop all` button, edited in place on each `share.changed` and turned
//! into [`STOPPED_TEXT`] (then unpinned and forgotten) once nothing is left; plus the owner-only
//! `/share`, `/share list`, `/share stop [id|all]`. Stop is never gated by an approval.
//!
//! The text is the pure [`share_status_text`]; [`LiveMessage`] is the whole message state, so
//! both are testable without Telegram. The channel feeds records in and performs the [`Step`]s.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use chrono::{Local, TimeZone};
use share::model::{Share, ShareState, StoppedBy};

use crate::api::{Button, Keyboard, MsgRef};

/// What the live message becomes when nothing is shared any more.
pub const STOPPED_TEXT: &str = "Public sharing stopped";
/// The `/share` reply when this engine runs no share plugin.
pub const NOT_RUNNING: &str = "sharing is not running on this engine";
const USAGE: &str = "usage: /share [list | stop [<id>|all]]";
/// The live message's button: a `cmd:` press runs `/share stop`.
const STOP_ALL_DATA: &str = "cmd:share:stop";

/// Counts as live for the indicators (UI1): Live, Offline or Pending.
#[must_use]
pub fn active(s: &Share) -> bool {
    matches!(
        s.state,
        ShareState::Live | ShareState::Offline | ShareState::Pending
    )
}

/// The live message for `shares` at `now_ms` (local clock), `None` when none is active.
#[must_use]
pub fn share_status_text(shares: &[Share], now_ms: u64) -> Option<String> {
    render(shares.iter(), now_ms, &Local, false)
}

/// `● Sharing publicly · N live`, one line per active share (oldest first), the stop hint.
/// `ids` adds each share's id for `/share stop <id>`.
fn render<'a, Tz>(
    shares: impl Iterator<Item = &'a Share>,
    now_ms: u64,
    tz: &Tz,
    ids: bool,
) -> Option<String>
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
        out.push('\n');
        out.push_str(&line(s, now_ms, tz));
        if ids {
            out.push_str(&format!(" · `{}`", s.id));
        }
    }
    out.push_str(if ids {
        "\n\n/share stop stops everything · /share stop <id> stops one"
    } else {
        "\n\n/share stop stops everything · /share list shows ids"
    });
    Some(out)
}

/// `https://x.tun.zip/ — expires 14:02 (in 3h)`, the state first when not yet / no longer live.
fn line<Tz>(s: &Share, now_ms: u64, tz: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    let state = match s.state {
        ShareState::Pending => "pending · ",
        ShareState::Offline => "offline · ",
        ShareState::Live | ShareState::Stopped => "",
    };
    let expiry = s
        .expires_ms
        .map_or_else(|| "no expiry".to_owned(), |e| expires(e, now_ms, tz));
    format!("{} — {state}{expiry}", s.url)
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
    format!("expires {clock} (in {})", span(at_ms - now_ms))
}

/// `<1m`, `45m`, `3h`, `2d`, rounded to the nearest unit.
fn span(ms: u64) -> String {
    let m = ms / 60_000;
    if m < 1 {
        "<1m".to_owned()
    } else if m < 60 {
        format!("{m}m")
    } else if m < 48 * 60 {
        format!("{}h", (m + 30) / 60)
    } else {
        format!("{}d", (m + 12 * 60) / (24 * 60))
    }
}

/// The live message's keyboard: one `Stop all` press.
#[must_use]
pub fn keyboard() -> Keyboard {
    Keyboard {
        rows: vec![vec![Button::Callback {
            text: "Stop all".to_owned(),
            data: STOP_ALL_DATA.to_owned(),
        }]],
        force_reply: false,
    }
}

/// What the channel does to the owner's chat after a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing changed.
    Idle,
    /// The first active share: send this text, pin it, then [`LiveMessage::posted`].
    Post(String),
    /// Edit the message in place.
    Edit(MsgRef, String),
    /// Nothing left: edit to [`STOPPED_TEXT`], unpin; already forgotten.
    End(MsgRef),
}

/// The owner's live message: the latest record of every share seen (the replicated rows of
/// other machines too) and where the message is.
// ponytail: stopped records stay for the process's life so a late seed never revives one.
#[derive(Debug, Default)]
pub struct LiveMessage {
    shares: BTreeMap<String, Share>,
    at: Option<MsgRef>,
    shown: String,
}

impl LiveMessage {
    /// Picks up a message a previous process left (edited or ended at the next step).
    #[must_use]
    pub fn new(at: Option<MsgRef>) -> Self {
        Self {
            at,
            ..Self::default()
        }
    }

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

    /// What to do now; assumes the edit lands (a failed edit is retried by the next change).
    pub fn step(&mut self, now_ms: u64) -> Step {
        self.step_in(now_ms, &Local)
    }

    fn step_in<Tz>(&mut self, now_ms: u64, tz: &Tz) -> Step
    where
        Tz: TimeZone,
        Tz::Offset: Display,
    {
        match (self.at, render(self.shares.values(), now_ms, tz, false)) {
            (None, None) => Step::Idle,
            (None, Some(text)) => Step::Post(text),
            (Some(_), Some(text)) if text == self.shown => Step::Idle,
            (Some(at), Some(text)) => {
                self.shown.clone_from(&text);
                Step::Edit(at, text)
            }
            (Some(at), None) => {
                self.at = None;
                self.shown.clear();
                Step::End(at)
            }
        }
    }

    /// A [`Step::Post`] landed at `at`.
    pub fn posted(&mut self, at: MsgRef, text: String) {
        self.at = Some(at);
        self.shown = text;
    }

    /// Where the message is, for persisting.
    #[must_use]
    pub fn at(&self) -> Option<MsgRef> {
        self.at
    }
}

/// `<data_dir>/telegram/share_message.json`: `[chat, message_id]` of the live message.
#[must_use]
pub fn message_path(data_dir: &Path) -> PathBuf {
    data_dir.join("telegram").join("share_message.json")
}

/// The message a previous process left, if any.
#[must_use]
pub fn load_message(path: &Path) -> Option<MsgRef> {
    let (chat, id): (i64, i64) = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(MsgRef { chat, id })
}

/// Remembers `at` across restarts; `None` forgets it.
pub fn save_message(path: &Path, at: Option<MsgRef>) {
    let r = match at {
        Some(at) => path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::write(
                    path,
                    serde_json::to_vec(&(at.chat, at.id)).unwrap_or_default(),
                )
            }),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
    };
    if let Err(e) = r {
        tracing::warn!(path = %path.display(), error = %e, "telegram: live-share message state");
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
        return Ok(render(list.iter(), now_ms(), &Local, true)
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
pub(crate) mod tests {
    use chrono::Utc;
    use share::model::{Access, ApprovedBy, CreatedBy, Source};

    use super::*;

    /// 2026-11-23 11:00 UTC.
    const NOW: u64 = 1_795_431_600_000;
    const HOUR: u64 = 3_600_000;

    pub(crate) fn share(id: &str, state: ShareState, expires_ms: Option<u64>) -> Share {
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
            live_ms: (state == ShareState::Live).then_some(NOW - HOUR),
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
    fn status_text_for_zero_one_and_two_shares() {
        assert_eq!(render([].iter(), NOW, &Utc, false), None);
        let stopped = share("s", ShareState::Stopped, Some(NOW + HOUR));
        assert_eq!(render([stopped.clone()].iter(), NOW, &Utc, false), None);
        assert_eq!(share_status_text(&[stopped], NOW), None);

        let x = share("x", ShareState::Live, Some(NOW + 3 * HOUR));
        assert_eq!(
            render([x.clone()].iter(), NOW, &Utc, false).unwrap(),
            "● Sharing publicly · 1 live\n\
             https://x.tun.zip/ — expires 14:00 (in 3h)\n\n\
             /share stop stops everything · /share list shows ids"
        );

        let mut y = share(
            "y",
            ShareState::Pending,
            Some(NOW + 26 * HOUR + 30 * 60_000),
        );
        y.created_ms = NOW;
        let z = share("z", ShareState::Offline, None);
        let text = render([y, x, z].iter(), NOW, &Utc, true).unwrap();
        assert_eq!(
            text,
            "● Sharing publicly · 3 live\n\
             https://x.tun.zip/ — expires 14:00 (in 3h) · `x`\n\
             https://z.tun.zip/ — offline · no expiry · `z`\n\
             https://y.tun.zip/ — pending · expires Nov 24 13:30 (in 27h) · `y`\n\n\
             /share stop stops everything · /share stop <id> stops one"
        );
        assert_eq!(span(30_000), "<1m");
        assert_eq!(span(45 * 60_000), "45m");
        assert_eq!(span(3 * 24 * HOUR), "3d");
    }

    #[test]
    fn live_message_posts_edits_and_ends() {
        let mut m = LiveMessage::default();
        assert_eq!(m.step_in(NOW, &Utc), Step::Idle);
        m.apply(share("x", ShareState::Pending, Some(NOW + HOUR)));
        let Step::Post(text) = m.step_in(NOW, &Utc) else {
            panic!("first active share posts")
        };
        assert!(text.contains("pending · "), "{text}");
        let at = MsgRef { chat: 1, id: 9 };
        m.posted(at, text);
        assert_eq!(m.step_in(NOW, &Utc), Step::Idle);
        m.apply(share("x", ShareState::Live, Some(NOW + HOUR)));
        let Step::Edit(e, text) = m.step_in(NOW, &Utc) else {
            panic!("a change edits in place")
        };
        assert_eq!(e, at);
        assert!(
            text.contains("1 live") && !text.contains("pending"),
            "{text}"
        );
        // A stale seed never revives what an event already brought.
        m.seed(vec![share("x", ShareState::Pending, None)]);
        assert_eq!(m.step_in(NOW, &Utc), Step::Idle);
        m.apply(share("x", ShareState::Stopped, Some(NOW + HOUR)));
        assert_eq!(m.step_in(NOW, &Utc), Step::End(at));
        assert_eq!(m.at(), None);
        assert_eq!(m.step_in(NOW, &Utc), Step::Idle);
        // A message left by a previous process is ended when nothing is live.
        let mut m = LiveMessage::new(Some(at));
        m.seed(vec![]);
        assert_eq!(m.step_in(NOW, &Utc), Step::End(at));
    }

    #[test]
    fn message_ref_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = message_path(dir.path());
        assert_eq!(load_message(&path), None);
        let at = MsgRef { chat: 5, id: 77 };
        save_message(&path, Some(at));
        assert_eq!(load_message(&path), Some(at));
        save_message(&path, None);
        assert_eq!(load_message(&path), None);
        save_message(&path, None);
    }

    #[test]
    fn stop_targets_one_active_share() {
        let list = vec![
            share("0193aa01", ShareState::Live, None),
            share("0193aa02", ShareState::Live, None),
            share("0193bb01", ShareState::Stopped, None),
        ];
        assert_eq!(pick(&list, "0193aa01").unwrap(), "0193aa01");
        assert_eq!(pick(&list, "https://0193aa02.tun.zip").unwrap(), "0193aa02");
        assert!(pick(&list, "0193aa").unwrap_err().contains("matches 2"));
        assert!(
            pick(&list, "0193bb")
                .unwrap_err()
                .starts_with("no live share")
        );
        assert_eq!(
            stop_all_text(2, Some("detached")),
            "Stopped all public sharing (2 on this machine).\nThe sharing edge has not confirmed yet (detached); it is told again when this machine reconnects."
        );
    }

    #[tokio::test]
    async fn commands_without_a_running_engine_say_so() {
        for arg in ["", "list", "stop", "stop all", "stop 0193aa01"] {
            assert_eq!(command(arg).await.unwrap(), NOT_RUNNING, "{arg}");
        }
        assert_eq!(command("frobnicate").await.unwrap(), USAGE);
        assert_eq!(command("list extra").await.unwrap(), USAGE);
    }
}
