//! Thread key ↔ session bindings, topics and the list of edit-mode messages
//! in flight, persisted at `<data_dir>/telegram/threads.json` on every change
//! (design doc 98 §6, §11).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runtime::SessionId;
use serde::{Deserialize, Serialize};

use crate::api::Target;

/// `"<chat>"` (flat chat or General) or `"<chat>:<topic>"`.
pub type ThreadKey = String;

/// The key for `thread` in `chat`.
#[must_use]
pub fn key(chat: i64, thread: Option<i64>) -> ThreadKey {
    match thread {
        Some(t) => format!("{chat}:{t}"),
        None => chat.to_string(),
    }
}

/// Parses a key back into a [`Target`]; `None` for garbage.
#[must_use]
pub fn target(key: &str) -> Option<Target> {
    let (chat, thread) = match key.split_once(':') {
        Some((c, t)) => (c, Some(t.parse().ok()?)),
        None => (key, None),
    };
    Some(Target {
        chat: chat.parse().ok()?,
        thread,
    })
}

/// A forum topic bound to a thread (98 §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    /// `message_thread_id`.
    pub id: i64,
    /// The user named it; never rewritten from the session title.
    pub named_by_user: bool,
}

/// One bound thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// The session this thread renders.
    pub session: SessionId,
    /// Telegram user who owns the thread.
    pub user: u64,
    /// Last title the topic was renamed to, if any.
    #[serde(default)]
    pub title: Option<String>,
    /// The topic, in topics chats.
    #[serde(default)]
    pub topic: Option<Topic>,
    /// Unix milliseconds of the last bind / title / touch; orders
    /// [`Threads::latest_in_chat`].
    #[serde(default)]
    pub updated_at: i64,
}

impl Thread {
    /// A fresh binding; `bind` stamps `updated_at`.
    #[must_use]
    pub fn new(session: SessionId, user: u64, topic: Option<Topic>) -> Self {
        Self {
            session,
            user,
            title: None,
            topic,
            updated_at: 0,
        }
    }
}

/// An edit-mode message mid-edit; repaired on boot (98 §11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenMsg {
    /// Chat id.
    pub chat: i64,
    /// Topic id, if any.
    pub thread: Option<i64>,
    /// The message.
    pub message_id: i64,
}

/// The file's shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    threads: BTreeMap<ThreadKey, Thread>,
    #[serde(default)]
    open: Vec<OpenMsg>,
}

/// Thread bindings plus open messages, saved after every mutation.
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
                tracing::warn!(path = %path.display(), error = %e, "corrupt telegram threads file; starting empty");
                File::default()
            }),
            Err(_) => File::default(),
        };
        Self {
            path: path.to_path_buf(),
            file,
        }
    }

    /// Writes the file (best effort, logged).
    pub fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let result = serde_json::to_vec_pretty(&self.file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&self.path, bytes));
        if let Err(e) = result {
            tracing::warn!(path = %self.path.display(), error = %e, "failed to save telegram threads");
        }
    }

    /// The thread at `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Thread> {
        self.file.threads.get(key)
    }

    /// Binds `key` to `thread`, dropping any other key bound to the same
    /// session (one session ↔ one thread); saves.
    pub fn bind(&mut self, key: ThreadKey, mut thread: Thread) {
        self.file
            .threads
            .retain(|k, v| *k == key || v.session != thread.session);
        thread.updated_at = now_ms();
        self.file.threads.insert(key, thread);
        self.save();
    }

    /// Drops `key`; saves when it existed.
    pub fn unbind(&mut self, key: &str) -> Option<Thread> {
        let t = self.file.threads.remove(key);
        if t.is_some() {
            self.save();
        }
        t
    }

    /// The thread rendering `session`.
    #[must_use]
    pub fn by_session(&self, session: SessionId) -> Option<(&ThreadKey, &Thread)> {
        self.file.threads.iter().find(|(_, t)| t.session == session)
    }

    /// Where `session`'s messages go.
    #[must_use]
    pub fn target_for(&self, session: SessionId) -> Option<Target> {
        self.by_session(session).and_then(|(k, _)| target(k))
    }

    /// `user`'s most recently active thread in `chat` (98 §10.3).
    #[must_use]
    pub fn latest_in_chat(&self, chat: i64, user: u64) -> Option<(&ThreadKey, &Thread)> {
        self.file
            .threads
            .iter()
            .filter(|(k, t)| t.user == user && target(k).is_some_and(|tg| tg.chat == chat))
            .max_by_key(|(_, t)| t.updated_at)
    }

    /// Marks `key` active now; saves when the key exists.
    pub fn touch(&mut self, key: &str) {
        if let Some(t) = self.file.threads.get_mut(key) {
            t.updated_at = now_ms();
            self.save();
        }
    }

    /// Sets `key`'s title and bumps `updated_at`; saves when the key exists.
    pub fn set_title(&mut self, key: &str, title: String) {
        if let Some(t) = self.file.threads.get_mut(key) {
            t.title = Some(title);
            t.updated_at = now_ms();
            self.save();
        }
    }

    /// Flags the topic as user-named; saves when the key has a topic.
    pub fn set_named_by_user(&mut self, key: &str, v: bool) {
        if let Some(topic) = self
            .file
            .threads
            .get_mut(key)
            .and_then(|t| t.topic.as_mut())
        {
            topic.named_by_user = v;
            self.save();
        }
    }

    /// Records an edit-mode message in flight; saves.
    pub fn open_add(&mut self, m: OpenMsg) {
        if !self.file.open.contains(&m) {
            self.file.open.push(m);
            self.save();
        }
    }

    /// Forgets a finished edit-mode message; saves when it was listed.
    pub fn open_remove(&mut self, m: &OpenMsg) {
        let before = self.file.open.len();
        self.file.open.retain(|o| o != m);
        if self.file.open.len() != before {
            self.save();
        }
    }

    /// Drains the open list for boot repair; saves.
    pub fn open_take(&mut self) -> Vec<OpenMsg> {
        let out = std::mem::take(&mut self.file.open);
        if !out.is_empty() {
            self.save();
        }
        out
    }

    /// Open messages currently listed (`TelegramStatus.open`).
    #[must_use]
    pub fn open_count(&self) -> u32 {
        u32::try_from(self.file.open.len()).unwrap_or(u32::MAX)
    }

    /// Every binding as `(key, session)`.
    #[must_use]
    pub fn list(&self) -> Vec<(String, SessionId)> {
        self.file
            .threads
            .iter()
            .map(|(k, t)| (k.clone(), t.session))
            .collect()
    }

    /// Drops every thread in `chat` (the bot was kicked, 98 §7); saves when
    /// anything went.
    pub fn unbind_chat(&mut self, chat: i64) -> Vec<Thread> {
        let gone: Vec<ThreadKey> = self
            .file
            .threads
            .keys()
            .filter(|k| target(k).is_some_and(|t| t.chat == chat))
            .cloned()
            .collect();
        let out: Vec<Thread> = gone
            .iter()
            .filter_map(|k| self.file.threads.remove(k))
            .collect();
        if !out.is_empty() {
            self.save();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `$TMPDIR/theuth-telegram-<rand>/telegram/threads.json`; the dir goes when the guard drops (141 R4).
    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("theuth-telegram-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("telegram").join("threads.json");
        (dir, path)
    }

    fn thread(user: u64) -> Thread {
        Thread::new(SessionId(uuid::Uuid::new_v4()), user, None)
    }

    #[test]
    fn keys_round_trip() {
        assert_eq!(key(7, None), "7");
        assert_eq!(key(-100, Some(5)), "-100:5");
        assert_eq!(
            target("-100:5"),
            Some(Target {
                chat: -100,
                thread: Some(5)
            })
        );
        assert_eq!(
            target("7"),
            Some(Target {
                chat: 7,
                thread: None
            })
        );
        assert_eq!(target("x:1"), None);
        assert_eq!(target("1:y"), None);
    }

    #[test]
    fn round_trips_threads_open_and_title() {
        let (_td, path) = tmp();
        let k = key(7, Some(3));
        let t = Thread::new(
            SessionId(uuid::Uuid::new_v4()),
            11,
            Some(Topic {
                id: 3,
                named_by_user: false,
            }),
        );
        let open = OpenMsg {
            chat: 7,
            thread: Some(3),
            message_id: 40,
        };
        {
            let mut th = Threads::load(&path);
            th.bind(k.clone(), t.clone());
            th.set_title(&k, "hello".into());
            th.set_named_by_user(&k, true);
            th.open_add(open.clone());
            th.open_add(open.clone());
        }
        let mut th = Threads::load(&path);
        let got = th.get(&k).expect("bound");
        assert_eq!(got.session, t.session);
        assert_eq!(got.user, 11);
        assert_eq!(got.title.as_deref(), Some("hello"));
        assert_eq!(got.topic.map(|t| t.named_by_user), Some(true));
        assert!(got.updated_at > 0);
        assert_eq!(th.by_session(t.session).map(|(k, _)| k), Some(&k));
        assert_eq!(
            th.target_for(t.session),
            Some(Target {
                chat: 7,
                thread: Some(3)
            })
        );
        assert_eq!(th.list(), vec![(k.clone(), t.session)]);
        assert_eq!(th.open_count(), 1);
        assert_eq!(th.open_take(), vec![open]);
        assert_eq!(Threads::load(&path).open_count(), 0);
        assert!(th.unbind(&k).is_some());
        assert!(th.unbind(&k).is_none());
        assert!(Threads::load(&path).get(&k).is_none());
    }

    #[test]
    fn corrupt_file_starts_empty() {
        let (_td, path) = tmp();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        let th = Threads::load(&path);
        assert!(th.list().is_empty());
        assert_eq!(th.open_count(), 0);
    }

    #[test]
    fn bind_replaces_older_key_for_same_session() {
        let (_td, path) = tmp();
        let mut th = Threads::load(&path);
        let t = thread(1);
        th.bind(key(7, Some(1)), t.clone());
        th.bind(key(7, Some(2)), t.clone());
        assert_eq!(th.list().len(), 1);
        assert!(th.get("7:1").is_none());
        assert_eq!(
            th.by_session(t.session).map(|(k, _)| k.as_str()),
            Some("7:2")
        );
    }

    #[test]
    fn latest_in_chat_and_unbind_chat() {
        let (_td, path) = tmp();
        let mut th = Threads::load(&path);
        th.bind(key(7, Some(1)), thread(1));
        th.bind(key(7, Some(2)), thread(2));
        th.bind(key(8, None), thread(1));
        let mut old = thread(1);
        old.updated_at = 0;
        th.bind(key(7, Some(0)), old);
        th.touch("7:1");
        let newest = th.get("7:1").unwrap().updated_at;
        th.file.threads.get_mut("7:0").unwrap().updated_at = newest - 1;
        assert_eq!(
            th.latest_in_chat(7, 1).map(|(k, _)| k.as_str()),
            Some("7:1")
        );
        assert_eq!(
            th.latest_in_chat(7, 2).map(|(k, _)| k.as_str()),
            Some("7:2")
        );
        assert!(th.latest_in_chat(9, 1).is_none());
        assert_eq!(th.unbind_chat(7).len(), 3);
        assert_eq!(th.list().len(), 1);
        assert!(th.get("8").is_some());
    }
}
