//! Thread ↔ session bindings and the open-stream list, persisted at
//! `<data_dir>/slack/threads.json` on every change (design doc 21 §6, §11).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// `"C…:1699999.123456"` — a channel and the thread's root `ts`.
pub type ThreadKey = String;

/// The key for `thread_ts` in `channel`.
#[must_use]
pub fn thread_key(channel: &str, thread_ts: &str) -> ThreadKey {
    format!("{channel}:{thread_ts}")
}

/// One bound thread (21 §6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// The session this thread renders.
    pub session: SessionId,
    /// The Slack user who owns the thread.
    pub user: String,
    /// Team id, for `chat.startStream`.
    pub team: String,
    /// Last title the thread was renamed to, if any.
    #[serde(default)]
    pub title: Option<String>,
    /// Unix milliseconds of the last bind / title / touch; orders
    /// [`Threads::latest_for`].
    #[serde(default)]
    pub updated_at: i64,
    /// 67 §10: the roster agent this thread is addressed to; `None` = theuth.
    #[serde(default)]
    pub agent: Option<String>,
    /// The agent's title, prefixed to each turn (`Title — …`).
    #[serde(default)]
    pub agent_title: Option<String>,
    /// The task the bound agent works from (`TaskId` as string).
    #[serde(default)]
    pub task: Option<String>,
}

impl Thread {
    /// A fresh theuth-bound thread; `bind` stamps `updated_at`.
    #[must_use]
    pub fn new(session: SessionId, user: String, team: String) -> Self {
        Self {
            session,
            user,
            team,
            title: None,
            updated_at: 0,
            agent: None,
            agent_title: None,
            task: None,
        }
    }
}

/// A stream left open by a live turn; stopped on boot (21 §11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stream {
    channel: String,
    ts: String,
}

/// The file's shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    threads: BTreeMap<ThreadKey, Thread>,
    #[serde(default)]
    streams: Vec<Stream>,
}

/// Thread bindings plus open streams, saved after every mutation.
#[derive(Debug)]
pub struct Threads {
    path: PathBuf,
    file: File,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Threads {
    /// Reads `path`; missing or corrupt → empty (corrupt logs a warning).
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let file = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "corrupt slack threads file; starting empty");
                File::default()
            }),
            Err(_) => File::default(),
        };
        Self {
            path: path.to_path_buf(),
            file,
        }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let result = serde_json::to_vec_pretty(&self.file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&self.path, bytes));
        if let Err(e) = result {
            tracing::warn!(path = %self.path.display(), error = %e, "failed to save slack threads");
        }
    }

    /// The thread at `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Thread> {
        self.file.threads.get(key)
    }

    /// The thread rendering `session`; the most recently active one when
    /// several share it (an agent's desk, 67 §10).
    // ponytail: desk replies go to the agent's most recently active thread; per-turn attribution if it misroutes.
    #[must_use]
    pub fn by_session(&self, session: SessionId) -> Option<(&ThreadKey, &Thread)> {
        self.file
            .threads
            .iter()
            .filter(|(_, t)| t.session == session)
            .max_by_key(|(_, t)| t.updated_at)
    }

    /// Binds `key` to `t`, dropping any other key bound to the same session
    /// (one session ↔ one thread) unless either side is agent-bound (a desk
    /// session is shared by its threads); saves.
    pub fn bind(&mut self, key: ThreadKey, mut t: Thread) {
        self.file.threads.retain(|k, v| {
            *k == key || v.session != t.session || v.agent.is_some() || t.agent.is_some()
        });
        t.updated_at = now_ms();
        self.file.threads.insert(key, t);
        self.save();
    }

    /// Drops `key`; saves when it existed.
    pub fn unbind(&mut self, key: &str) {
        if self.file.threads.remove(key).is_some() {
            self.save();
        }
    }

    /// Sets `key`'s title and bumps `updated_at`; saves when the key exists.
    pub fn set_title(&mut self, key: &str, title: Option<String>) {
        if let Some(t) = self.file.threads.get_mut(key) {
            t.title = title;
            t.updated_at = now_ms();
            self.save();
        }
    }

    /// Marks `key` active now (a message arrived or a turn ran); saves when
    /// the key exists.
    pub fn touch(&mut self, key: &str) {
        if let Some(t) = self.file.threads.get_mut(key) {
            t.updated_at = now_ms();
            self.save();
        }
    }

    /// `user`'s most recently active thread in `channel` (21 §10.2).
    #[must_use]
    pub fn latest_for(&self, channel: &str, user: &str) -> Option<(&ThreadKey, &Thread)> {
        let prefix = format!("{channel}:");
        self.file
            .threads
            .iter()
            .filter(|(k, t)| k.starts_with(&prefix) && t.user == user)
            .max_by_key(|(_, t)| t.updated_at)
    }

    /// Records an open stream; saves.
    pub fn add_stream(&mut self, channel: &str, ts: &str) {
        let s = Stream {
            channel: channel.to_owned(),
            ts: ts.to_owned(),
        };
        if !self.file.streams.contains(&s) {
            self.file.streams.push(s);
            self.save();
        }
    }

    /// Forgets a stream that was stopped; saves when it was listed.
    pub fn remove_stream(&mut self, channel: &str, ts: &str) {
        let before = self.file.streams.len();
        self.file
            .streams
            .retain(|s| !(s.channel == channel && s.ts == ts));
        if self.file.streams.len() != before {
            self.save();
        }
    }

    /// Streams currently listed as open (21 §13 `open_streams`).
    #[must_use]
    pub fn open_streams(&self) -> u32 {
        u32::try_from(self.file.streams.len()).unwrap_or(u32::MAX)
    }

    /// Drains the open-stream list for boot cleanup; saves.
    pub fn take_streams(&mut self) -> Vec<(String, String)> {
        let out: Vec<(String, String)> = self
            .file
            .streams
            .drain(..)
            .map(|s| (s.channel, s.ts))
            .collect();
        if !out.is_empty() {
            self.save();
        }
        out
    }

    /// Every binding as `(key, session)`.
    #[must_use]
    pub fn all(&self) -> Vec<(ThreadKey, SessionId)> {
        self.file
            .threads
            .iter()
            .map(|(k, t)| (k.clone(), t.session))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `$TMPDIR/theuth-slack-<rand>/slack/threads.json`; the dir goes when the guard drops (141 R4).
    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("theuth-slack-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("slack").join("threads.json");
        (dir, path)
    }

    fn thread(user: &str) -> Thread {
        Thread::new(SessionId(uuid::Uuid::new_v4()), user.into(), "T1".into())
    }

    #[test]
    fn round_trips_threads_streams_and_title() {
        let (_td, path) = tmp();
        let key = thread_key("C1", "1.000");
        let t = thread("U1");
        {
            let mut th = Threads::load(&path);
            th.bind(key.clone(), t.clone());
            th.set_title(&key, Some("hello".into()));
            th.add_stream("C1", "1.001");
            th.add_stream("C1", "1.001");
        }
        let mut th = Threads::load(&path);
        let got = th.get(&key).expect("bound");
        assert_eq!(got.session, t.session);
        assert_eq!(got.title.as_deref(), Some("hello"));
        assert!(got.updated_at > 0);
        assert_eq!(th.by_session(t.session).map(|(k, _)| k), Some(&key));
        assert_eq!(th.all(), vec![(key.clone(), t.session)]);
        assert_eq!(
            th.take_streams(),
            vec![("C1".to_owned(), "1.001".to_owned())]
        );
        assert!(Threads::load(&path).take_streams().is_empty());
        th.unbind(&key);
        assert!(Threads::load(&path).get(&key).is_none());
    }

    #[test]
    fn corrupt_file_starts_empty() {
        let (_td, path) = tmp();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        let th = Threads::load(&path);
        assert!(th.all().is_empty());
        assert_eq!(th.open_streams(), 0);
    }

    #[test]
    fn bind_replaces_older_key_for_same_session() {
        let (_td, path) = tmp();
        let mut th = Threads::load(&path);
        let t = thread("U1");
        th.bind(thread_key("C1", "1.000"), t.clone());
        th.bind(thread_key("C1", "2.000"), t.clone());
        assert_eq!(th.all().len(), 1);
        assert!(th.get("C1:1.000").is_none());
        assert_eq!(
            th.by_session(t.session).map(|(k, _)| k.as_str()),
            Some("C1:2.000")
        );
        // Rebinding the same key keeps it.
        th.bind(thread_key("C1", "2.000"), t.clone());
        assert_eq!(th.all().len(), 1);
    }

    #[test]
    fn agent_binding_persists() {
        let (_td, path) = tmp();
        let desk = SessionId(uuid::Uuid::new_v4());
        let mut a = Thread::new(desk, "U1".into(), "T1".into());
        a.agent = Some("ops".into());
        a.agent_title = Some("Ops".into());
        a.task = Some("t-1".into());
        let mut b = a.clone();
        b.task = Some("t-2".into());
        {
            let mut th = Threads::load(&path);
            th.bind(thread_key("C1", "1.000"), a.clone());
            th.bind(thread_key("C1", "2.000"), b);
            // Two agent threads share the desk session: neither evicts the other.
            assert_eq!(th.all().len(), 2);
            th.touch("C1:1.000");
            let newest = th.file.threads["C1:1.000"].updated_at;
            th.file.threads.get_mut("C1:2.000").unwrap().updated_at = newest - 1;
        }
        let th = Threads::load(&path);
        let got = th.get("C1:1.000").expect("bound");
        assert_eq!(got.agent.as_deref(), Some("ops"));
        assert_eq!(got.agent_title.as_deref(), Some("Ops"));
        assert_eq!(got.task.as_deref(), Some("t-1"));
        assert_eq!(
            th.by_session(desk).map(|(k, _)| k.as_str()),
            Some("C1:1.000")
        );
        // A theuth thread still evicts an older theuth thread of the same session.
        let mut th = th;
        let t = thread("U1");
        th.bind(thread_key("C2", "1.000"), t.clone());
        th.bind(thread_key("C2", "2.000"), t);
        assert!(th.get("C2:1.000").is_none());
    }

    #[test]
    fn latest_for_picks_newest_of_user_in_channel() {
        let (_td, path) = tmp();
        let mut th = Threads::load(&path);
        th.bind(thread_key("C1", "1.000"), thread("U1"));
        th.bind(thread_key("C1", "2.000"), thread("U2"));
        th.bind(thread_key("C2", "3.000"), thread("U1"));
        let mut old = thread("U1");
        old.updated_at = 0;
        th.bind(thread_key("C1", "0.500"), old);
        // Bind stamps now; make the first one explicitly newest.
        th.touch("C1:1.000");
        let newest = th.file.threads.get("C1:1.000").unwrap().updated_at;
        th.file.threads.get_mut("C1:0.500").unwrap().updated_at = newest - 1;
        assert_eq!(
            th.latest_for("C1", "U1").map(|(k, _)| k.as_str()),
            Some("C1:1.000")
        );
        assert_eq!(
            th.latest_for("C1", "U2").map(|(k, _)| k.as_str()),
            Some("C1:2.000")
        );
        assert!(th.latest_for("C3", "U1").is_none());
        assert!(th.latest_for("C1", "U9").is_none());
    }
}
