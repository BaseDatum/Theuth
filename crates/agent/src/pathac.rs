//! PATH-AC: path autocomplete for the composer. [`trigger`] finds the path
//! token under the cursor (ported verbatim to `lib/path-complete.ts` in the
//! desktop UI, same vectors); [`complete`] lists the one directory that token
//! names on the engine running the session. `~`, `~user` and `$VAR` are
//! expanded for the lookup only: every `insert` keeps the user's prefix.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Most candidates one reply carries; `truncated` says more existed.
pub const LIMIT: usize = 50;

/// Reply to `path_complete`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PathCompletion {
    /// Byte range of the token the candidates replace (`insert` goes over it).
    pub token_start: usize,
    /// End of that range (the cursor).
    pub token_end: usize,
    /// Directories first, then by name; hidden entries only for a `.` segment.
    pub candidates: Vec<PathCandidate>,
    /// More than [`LIMIT`] entries matched.
    pub truncated: bool,
    /// `std::env::consts::OS` of the engine that listed (`windows` enables `C:\` triggers).
    pub platform: String,
}

/// One entry of the completed directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PathCandidate {
    /// The whole token after accepting: the typed prefix verbatim plus the entry name (dirs end in `/`).
    pub insert: String,
    /// The entry name, `/`-suffixed for a directory.
    pub display: String,
    /// A directory: accepting keeps completing inside it.
    pub is_dir: bool,
}

/// The path token under `cursor`: `[start, cursor)` in bytes, when it starts
/// with `/`, `./`, `../`, `~`, `~user/`, `$VAR/` or (on Windows) `C:\`/`C:/`,
/// is not inside a fenced code block and does not follow `@`. A token
/// opened with `"` or `'` may contain spaces (the quote stays outside the
/// range); an unquoted one may contain `\ `.
pub fn trigger(input: &str, cursor: usize, windows: bool) -> Option<(usize, usize)> {
    let cursor = input.floor_char_boundary(cursor.min(input.len()));
    let before = &input[..cursor];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let fences = before[..line_start]
        .lines()
        .filter(|l| l.trim_start().starts_with("```"))
        .count();
    if fences % 2 == 1 {
        return None;
    }
    let line = &before[line_start..];
    // Quoted: the last `"`/`'` on the line that opens a word and is still open.
    let quoted = line
        .char_indices()
        .rev()
        .find(|&(i, c)| {
            (c == '"' || c == '\'')
                && line[..i]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace)
                && !line[i + 1..].contains(c)
        })
        .map(|(i, _)| i + 1);
    let start = match quoted {
        Some(q) => q,
        None => {
            let bytes = line.as_bytes();
            let mut i = line.len();
            while i > 0 {
                let c = line[..i].chars().next_back().unwrap();
                if c.is_whitespace() && !(c == ' ' && i >= 2 && bytes[i - 2] == b'\\') {
                    break;
                }
                i -= c.len_utf8();
            }
            i
        }
    };
    let token = &line[start..];
    if is_path(token, windows) {
        Some((line_start + start, cursor))
    } else {
        None
    }
}

/// The shapes that mean "a path" (shared with the TS port).
fn is_path(token: &str, windows: bool) -> bool {
    if token.starts_with('/') || token.starts_with("./") || token.starts_with("../") {
        return true;
    }
    if let Some(rest) = token.strip_prefix('~') {
        return rest.is_empty()
            || rest.starts_with('/')
            || rest.split_once('/').is_some_and(|(user, _)| {
                !user.is_empty()
                    && user
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
            });
    }
    if let Some(rest) = token.strip_prefix('$') {
        return rest.split_once('/').is_some_and(|(var, _)| {
            !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    }
    if windows {
        let b = token.as_bytes();
        return b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'/' || b[2] == b'\\');
    }
    false
}

/// `~`, `~user` and `$VAR` heads expanded for the lookup; anything relative
/// is joined onto `cwd`. `None`: the variable or user is unknown.
fn expand(dir: &str, cwd: &Path) -> Option<PathBuf> {
    let home = std::env::home_dir();
    let path = if let Some(rest) = dir.strip_prefix('~') {
        let (user, rest) = rest.split_once('/').unwrap_or((rest, ""));
        let base = if user.is_empty() {
            home?
        } else {
            // ponytail: `~user` = sibling of $HOME (/Users, /home); getpwnam if that ever misses.
            home?.parent()?.join(user)
        };
        base.join(rest)
    } else if let Some(rest) = dir.strip_prefix('$') {
        let (var, rest) = rest.split_once('/').unwrap_or((rest, ""));
        PathBuf::from(std::env::var_os(var)?).join(rest)
    } else {
        PathBuf::from(dir)
    };
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

/// Complete the token under `cursor` against the file system, relative
/// paths against `cwd`. Empty candidates when nothing triggers, the
/// directory is missing or unreadable, or the segment matches nothing.
pub async fn complete(cwd: &Path, input: &str, cursor: usize) -> PathCompletion {
    let platform = std::env::consts::OS.to_string();
    let Some((start, end)) = trigger(input, cursor, platform == "windows") else {
        return PathCompletion {
            platform,
            ..Default::default()
        };
    };
    let quoted = start > 0 && matches!(input.as_bytes()[start - 1], b'"' | b'\'');
    let token = &input[start..end];
    // The typed head stays verbatim; only `segment` is matched against names.
    let windows = platform == "windows";
    let sep = if windows && token.contains('\\') && !token.contains('/') {
        '\\'
    } else {
        '/'
    };
    let seps: &[char] = if windows { &['/', '\\'] } else { &['/'] };
    let (head, segment) = match token.rfind(seps) {
        Some(i) => token.split_at(i + 1),
        None => (token, ""), // only `~` (no separator) triggers without one
    };
    let head_insert = if head == "~" {
        "~/".to_string()
    } else {
        head.to_string()
    };
    let unescape = |s: &str| {
        if quoted {
            s.to_string()
        } else {
            s.replace("\\ ", " ")
        }
    };
    let dir = expand(&unescape(head), cwd).unwrap_or_default();
    let segment = unescape(segment);
    let mut out = PathCompletion {
        token_start: start,
        token_end: end,
        platform,
        ..Default::default()
    };
    let Ok(mut rd) = tokio::fs::read_dir(&dir).await else {
        return out;
    };
    let mut names: Vec<(bool, String)> = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(&segment) || name.chars().any(char::is_control) {
            continue;
        }
        if name.starts_with('.') && !segment.starts_with('.') {
            continue;
        }
        let is_dir = match entry.file_type().await {
            Ok(t) if t.is_symlink() => tokio::fs::metadata(entry.path())
                .await
                .is_ok_and(|m| m.is_dir()),
            Ok(t) => t.is_dir(),
            Err(_) => false,
        };
        names.push((is_dir, name.to_string()));
    }
    names.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    out.truncated = names.len() > LIMIT;
    names.truncate(LIMIT);
    out.candidates = names
        .into_iter()
        .map(|(is_dir, name)| {
            let tail = if is_dir {
                sep.to_string()
            } else {
                String::new()
            };
            let escaped = if quoted {
                name.clone()
            } else {
                name.replace(' ', "\\ ")
            };
            PathCandidate {
                insert: format!("{head_insert}{escaped}{tail}"),
                display: format!("{name}{tail}"),
                is_dir,
            }
        })
        .collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> (String, usize) {
        let i = s.find('|').expect("cursor");
        (s.replace('|', ""), i)
    }

    #[test]
    fn trigger_vectors() {
        let yes = [
            ("~/De|", (0, 4)),
            ("see ~/Develop/th|", (4, 16)),
            ("./src/|", (0, 6)),
            ("$HOME/.th|", (0, 9)),
            ("../x|", (0, 4)),
            ("/|", (0, 1)),
            ("~|", (0, 1)),
            ("~bob/x|", (0, 6)),
            ("open \"~/My Docs/f|", (6, 17)),
            ("a\nb /etc/ho|", (4, 11)),
            ("~/My\\ Docs/|", (0, 11)),
        ];
        for (s, want) in yes {
            let (text, cur) = at(s);
            assert_eq!(trigger(&text, cur, false), Some(want), "{s}");
        }
        let no = [
            "no/trigger here|",
            "no/trigger| here",
            "```\n~/De|",
            "```sh\nls ~/x\n```\n```\n/etc|",
            "@note|",
            "@~/x|",
            "hello|",
            "$HOME|",
            "~bob|",
            "C:\\Users\\|",
            "",
        ];
        for s in no {
            let (text, cur) = at(&format!("{s}{}", if s.contains('|') { "" } else { "|" }));
            assert_eq!(trigger(&text, cur, false), None, "{s}");
        }
        let (text, cur) = at("```\n~/x\n```\nsee ~/De|");
        assert_eq!(trigger(&text, cur, false), Some((16, 20)), "fence closed");
        assert_eq!(trigger("C:\\Users\\", 9, true), Some((0, 9)));
        assert_eq!(trigger("D:/x", 4, true), Some((0, 4)));
    }

    #[test]
    fn expand_is_lookup_only() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand("~/", Path::new("/cwd")), Some(home.clone()));
        assert_eq!(expand("~/a/", Path::new("/cwd")), Some(home.join("a/")));
        assert_eq!(expand("$HOME/b/", Path::new("/cwd")), Some(home.join("b/")));
        assert_eq!(
            expand("./s/", Path::new("/cwd")),
            Some(PathBuf::from("/cwd/./s/"))
        );
        assert_eq!(expand("$NO_SUCH_VAR_PATHAC/", Path::new("/cwd")), None);
    }

    #[tokio::test]
    async fn completes_one_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for d in ["Develop", "Documents", ".hidden", "zeta"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        for f in ["Dfile.txt", ".dotfile", "a b.txt"] {
            std::fs::write(root.join(f), b"").unwrap();
        }
        std::fs::write(root.join("Develop/nested"), b"").unwrap();
        let cwd = root;

        // Relative to cwd, dirs first then name, hidden filtered, `/` suffix; head kept verbatim.
        let r = complete(cwd, "see ./D", 7).await;
        assert_eq!((r.token_start, r.token_end), (4, 7));
        let shown: Vec<_> = r.candidates.iter().map(|c| c.display.as_str()).collect();
        assert_eq!(shown, ["Develop/", "Documents/", "Dfile.txt"]);
        assert_eq!(r.candidates[0].insert, "./Develop/");
        assert!(r.candidates[0].is_dir && !r.candidates[2].is_dir);
        assert!(!r.truncated);

        // Hidden only for a `.` segment.
        let r = complete(cwd, "./.", 3).await;
        let shown: Vec<_> = r.candidates.iter().map(|c| c.display.as_str()).collect();
        assert_eq!(shown, [".hidden/", ".dotfile"]);

        // Everything in the dir, spaces escaped when unquoted, kept when quoted.
        let r = complete(cwd, "./", 2).await;
        assert_eq!(r.candidates.len(), 5);
        assert!(r.candidates.iter().any(|c| c.insert == "./a\\ b.txt"));
        let r = complete(cwd, "\"./a", 4).await;
        assert_eq!(r.candidates[0].insert, "./a b.txt");
        let r = complete(cwd, "./a\\ b", 6).await;
        assert_eq!(r.candidates[0].insert, "./a\\ b.txt");

        // `~` and `$HOME` expand for lookup only: inserts keep the prefix.
        let home = std::env::home_dir().unwrap();
        let r = complete(cwd, "~/", 2).await;
        let mut real: Vec<_> = std::fs::read_dir(&home)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        real.sort();
        assert_eq!(r.candidates.len(), real.len().min(LIMIT));
        assert!(r.candidates.iter().all(|c| c.insert.starts_with("~/")));
        let bare = complete(cwd, "~", 1).await;
        assert_eq!(bare.candidates.len(), r.candidates.len());
        assert!(bare.candidates.iter().all(|c| c.insert.starts_with("~/")));
        let r2 = complete(cwd, "$HOME/", 6).await;
        assert!(r2.candidates.iter().all(|c| c.insert.starts_with("$HOME/")));
        assert_eq!(r2.candidates.len(), r.candidates.len());

        // Missing dir, no trigger, unknown var: empty, never an error.
        assert!(complete(cwd, "./nope/x", 8).await.candidates.is_empty());
        assert!(complete(cwd, "plain text", 10).await.candidates.is_empty());
        assert!(
            complete(cwd, "$NO_SUCH_VAR_PATHAC/x", 21)
                .await
                .candidates
                .is_empty()
        );
        assert_eq!(complete(cwd, "x", 1).await.platform, std::env::consts::OS);

        // Truncation at LIMIT.
        let many = tmp.path().join("many");
        std::fs::create_dir(&many).unwrap();
        for i in 0..(LIMIT + 5) {
            std::fs::write(many.join(format!("f{i:03}")), b"").unwrap();
        }
        let r = complete(cwd, "./many/", 7).await;
        assert!(r.truncated);
        assert_eq!(r.candidates.len(), LIMIT);
    }
}
