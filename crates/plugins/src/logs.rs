//! Per-plugin redacted log files (`<logs_dir>/<name>.log.<YYYY-MM-DD>`, design §11, §7.4), tail, sweep.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Local, NaiveDate, SecondsFormat};

/// One plugin's log: `<logs_dir>/<name>.log.<YYYY-MM-DD>` (local date), redacted with
/// `tools::secrets::redact`, capped at `max_bytes` per day (past it lines are counted and dropped;
/// one "… N lines dropped" line is appended at rollover and when the log is dropped).
pub(crate) struct PluginLog {
    dir: PathBuf,
    name: String,
    max_bytes: u64,
    state: Mutex<State>,
}

struct State {
    day: NaiveDate,
    file: Option<File>,
    /// Bytes in the current day's file (initialized from its size).
    written: u64,
    /// Lines dropped today past the cap.
    dropped: u64,
}

fn day_path(dir: &Path, name: &str, day: NaiveDate) -> PathBuf {
    dir.join(format!("{name}.log.{}", day.format("%Y-%m-%d")))
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// Today's log path of `name`.
pub(crate) fn path_today(logs_dir: &Path, name: &str) -> PathBuf {
    day_path(logs_dir, name, Local::now().date_naive())
}

impl PluginLog {
    /// A log for `name` under `logs_dir` (created on first write).
    pub fn new(logs_dir: &Path, name: &str, max_bytes: u64) -> Self {
        let day = Local::now().date_naive();
        let written = file_len(&day_path(logs_dir, name, day));
        Self {
            dir: logs_dir.to_path_buf(),
            name: name.to_string(),
            max_bytes,
            state: Mutex::new(State {
                day,
                file: None,
                written,
                dropped: 0,
            }),
        }
    }

    /// Today's file.
    pub fn path_today(&self) -> PathBuf {
        path_today(&self.dir, &self.name)
    }

    /// Append `"<RFC3339 local> [<source>] <text>\n"`; `source` ∈ host|stderr|stdout|log:<level>.
    pub fn line(&self, source: &str, text: &str) {
        self.line_at(Local::now(), source, text);
    }

    fn line_at(&self, now: DateTime<Local>, source: &str, text: &str) {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let day = now.date_naive();
        if day != st.day {
            self.flush_dropped(&mut st, now);
            st.day = day;
            st.file = None;
            st.written = file_len(&day_path(&self.dir, &self.name, day));
        }
        let clean = escape_controls(::tools::secrets::redact(text).trim_end_matches(['\n', '\r']));
        let line = format!(
            "{} [{source}] {clean}\n",
            now.to_rfc3339_opts(SecondsFormat::Secs, false),
        );
        if st.written.saturating_add(line.len() as u64) > self.max_bytes {
            st.dropped += 1;
            return;
        }
        if self.append(&mut st, &line) {
            st.written += line.len() as u64;
        }
    }

    /// Write the pending "… N lines dropped" line to the current day's file (past the cap).
    fn flush_dropped(&self, st: &mut State, now: DateTime<Local>) {
        if st.dropped == 0 {
            return;
        }
        let line = format!(
            "{} [host] … {} lines dropped (log_max_bytes {})\n",
            now.to_rfc3339_opts(SecondsFormat::Secs, false),
            st.dropped,
            self.max_bytes
        );
        st.dropped = 0;
        if self.append(st, &line) {
            st.written += line.len() as u64;
        }
    }

    fn append(&self, st: &mut State, line: &str) -> bool {
        if st.file.is_none() {
            let path = day_path(&self.dir, &self.name, st.day);
            let opened = std::fs::create_dir_all(&self.dir).and_then(|()| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
            });
            match opened {
                Ok(f) => st.file = Some(f),
                Err(e) => {
                    tracing::debug!(path = %path.display(), error = %e, "plugins: cannot open log");
                    return false;
                }
            }
        }
        let Some(f) = st.file.as_mut() else {
            return false;
        };
        match f.write_all(line.as_bytes()) {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!(plugin = %self.name, error = %e, "plugins: log write failed");
                st.file = None;
                false
            }
        }
    }
}

impl Drop for PluginLog {
    fn drop(&mut self) {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.flush_dropped(&mut st, Local::now());
    }
}

/// The last `n` lines of `path`; missing file → empty. Reads backwards in chunks.
pub(crate) fn tail(path: &Path, n: usize) -> Vec<String> {
    const CHUNK: u64 = 64 * 1024;
    if n == 0 {
        return Vec::new();
    }
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let mut pos = len;
    let mut buf: Vec<u8> = Vec::new();
    while pos > 0 && buf.iter().filter(|&&b| b == b'\n').count() <= n {
        let take = CHUNK.min(pos);
        pos -= take;
        let mut chunk = vec![0; take as usize];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut chunk).is_err() {
            break;
        }
        chunk.extend_from_slice(&buf);
        buf = chunk;
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|l| (*l).to_string()).collect()
}

/// Delete every `*.log.<YYYY-MM-DD>` in `logs_dir` dated more than `days` days before today.
pub(crate) fn sweep(logs_dir: &Path, days: u32) {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    let today = Local::now().date_naive();
    let Some(cutoff) = today.checked_sub_days(chrono::Days::new(u64::from(days))) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some((_, date)) = name.to_str().and_then(|s| s.rsplit_once(".log.")) else {
            continue;
        };
        let Ok(date) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
            continue;
        };
        if date < cutoff
            && entry.file_type().is_ok_and(|t| t.is_file())
            && let Err(e) = std::fs::remove_file(entry.path())
        {
            tracing::debug!(path = %entry.path().display(), error = %e, "plugins: log sweep");
        }
    }
}

/// Interior newlines, carriage returns and other control characters (tab kept) as Rust escapes,
/// so one plugin message is one log line and cannot forge `[host]` lines or repaint a terminal
/// (review R22).
fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && c != '\t' {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_drops_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = PluginLog::new(dir.path(), "turns", 200);
        for i in 0..20 {
            log.line("stderr", &format!("line {i}"));
        }
        let path = log.path_today();
        let size = file_len(&path);
        assert!(size > 0 && size <= 200, "{size}");
        let kept = tail(&path, 100).len();
        assert!(kept < 20);
        drop(log);
        let lines = tail(&path, 100);
        let last = lines.last().unwrap();
        assert!(
            last.contains(&format!("… {} lines dropped", 20 - kept)),
            "{last}"
        );
        // A new log on the same day starts from the file's size: nothing more fits.
        let again = PluginLog::new(dir.path(), "turns", 200);
        again.line("host", "more");
        assert_eq!(tail(&path, 100).len(), lines.len());
    }

    #[test]
    fn tail_returns_last_n() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log.2026-11-24");
        let text: String = (0..20_000).map(|i| format!("row {i}\n")).collect();
        std::fs::write(&path, text).unwrap();
        assert_eq!(tail(&path, 3), vec!["row 19997", "row 19998", "row 19999"]);
        assert_eq!(tail(&path, 0), Vec::<String>::new());
        assert_eq!(tail(&path, 50_000).len(), 20_000);
        assert!(tail(&dir.path().join("missing"), 5).is_empty());
    }

    #[test]
    fn redaction_hook_absent_leaves_text_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let log = PluginLog::new(dir.path(), "ping", 1 << 20);
        log.line("log:info", "plain text token=abc123\n");
        let lines = tail(&log.path_today(), 10);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].ends_with(" [log:info] plain text token=abc123"),
            "{}",
            lines[0]
        );
    }

    /// R22: an embedded newline cannot forge a second `[host]` line.
    #[test]
    fn interior_newlines_are_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let log = PluginLog::new(dir.path(), "ping", 1 << 20);
        log.line(
            "log:info",
            "ok\n2026-11-25T00:00:00+00:00 [host] approved via desktop\r\x1b[2K\n",
        );
        let lines = tail(&log.path_today(), 10);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].ends_with(
                " [log:info] ok\\n2026-11-25T00:00:00+00:00 [host] approved via desktop\\r\\u{1b}[2K"
            ),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn sweep_deletes_old_days() {
        let dir = tempfile::tempdir().unwrap();
        let today = Local::now().date_naive();
        let old = day_path(dir.path(), "turns", today - chrono::Days::new(10));
        let recent = day_path(dir.path(), "turns", today - chrono::Days::new(2));
        let other = dir.path().join("notes.txt");
        for p in [&old, &recent, &other] {
            std::fs::write(p, "x").unwrap();
        }
        sweep(dir.path(), 5);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(other.exists());
    }
}
