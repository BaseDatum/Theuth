//! Tests for [`crate::text`] (193): the tools end to end on real files.

use super::*;
use crate::edit::{Edit, EditArgs, EditOp};
use crate::fs::{ReadFile, ReadFileArgs, WriteFile, WriteFileArgs};
use crate::textref::{SymbolSpan, Syntax};
use std::sync::Arc;

/// A stand-in for codeindex's hook on `.fk` files: `fn NAME` / `type
/// NAME` lines open symbols spanning their deeper-indented lines, `///`
/// lines above widen them, a nested `fn` is `Type.fn`; lines after a line
/// with one `"""` through the next such line are string interiors. It
/// panics on `.boom` files (a failed hook).
struct Fake;

impl Syntax for Fake {
    fn analyze(&self, path: &Path, text: &str) -> Option<SyntaxInfo> {
        if path.extension()? == "boom" {
            panic!("fake parser blew up");
        }
        if path.extension()? != "fk" {
            return None;
        }
        let lines: Vec<&str> = text.lines().collect();
        let ind = |l: &str| l.len() - l.trim_start().len();
        let mut info = SyntaxInfo::default();
        let mut open: Vec<(usize, String)> = Vec::new();
        let mut in_str = false;
        for (i, l) in lines.iter().enumerate() {
            if in_str {
                info.string_lines.push(i + 1);
            }
            if l.matches("\"\"\"").count() == 1 {
                in_str = !in_str;
            }
            let t = l.trim_start();
            let Some(name) = t.strip_prefix("fn ").or_else(|| t.strip_prefix("type ")) else {
                continue;
            };
            let d = ind(l);
            open.retain(|(od, _)| *od < d);
            let mut q: Vec<&str> = open.iter().map(|(_, n)| n.as_str()).collect();
            q.push(name.trim());
            let mut end = i;
            for (j, m) in lines.iter().enumerate().skip(i + 1) {
                if m.trim().is_empty() {
                    continue;
                }
                if ind(m) <= d {
                    break;
                }
                end = j;
            }
            let mut start = i;
            while start > 0 && lines[start - 1].trim_start().starts_with("///") {
                start -= 1;
            }
            info.symbols.push(SymbolSpan {
                name: q.join("."),
                kind: "function".into(),
                start: start + 1,
                end: end + 1,
            });
            open.push((d, name.trim().to_owned()));
        }
        Some(info)
    }
}

struct T {
    dir: PathBuf,
    cx: ToolCx,
    _scratch: tempfile::TempDir,
}

fn t() -> T {
    crate::textref::set_syntax(Arc::new(Fake));
    let scratch = crate::test_dir("text");
    // An `.editorconfig` above the temp dir must not leak into tests.
    std::fs::write(scratch.path().join(".editorconfig"), "root = true\n").unwrap();
    T {
        dir: scratch.path().to_path_buf(),
        cx: ToolCx {
            session: crate::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
            cancel: tokio_util::sync::CancellationToken::new(),
            promote_after: None,
        },
        _scratch: scratch,
    }
}

fn id_of(s: &str) -> String {
    let i = s.find(", @").unwrap_or_else(|| panic!("no id in {s}"));
    s[i + 3..i + 11].to_owned()
}

/// The id a receipt reports for `p`.
fn receipt_id(s: &str, p: &Path) -> String {
    let key = format!("{} @", p.display());
    let i = s.rfind(&key).unwrap_or_else(|| panic!("no {key} in {s}"));
    s[i + key.len()..i + key.len() + 8].to_owned()
}

impl T {
    fn put(&self, name: &str, content: &str) -> PathBuf {
        let p = self.dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }
    fn get(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(name)).unwrap()
    }
    async fn read(&self, name: &str) -> String {
        Tool::call(
            &ReadFile,
            ReadFileArgs {
                path: self.dir.join(name),
                offset: None,
                limit: None,
                tail: None,
                ranges: None,
                numbers: true,
                outline: false,
            },
            self.cx.clone(),
        )
        .await
        .unwrap()
        .content
    }
    async fn id(&self, name: &str) -> String {
        id_of(&self.read(name).await)
    }
    fn r(&self, name: &str, anchor: &str) -> String {
        format!("{}#{anchor}", self.dir.join(name).display())
    }
    async fn paste(
        &self,
        cut: bool,
        src: &str,
        target: (&str, &str),
        indent: &str,
    ) -> anyhow::Result<ToolOutput> {
        let (kind, r) = target;
        let (mut before, mut after, mut at) = (None, None, None);
        match kind {
            "before" => before = Some(r.to_owned()),
            "after" => after = Some(r.to_owned()),
            _ => at = Some(r.to_owned()),
        }
        if cut {
            let a = MoveTextArgs {
                src: src.into(),
                before,
                after,
                at,
                indent: indent.into(),
            };
            Tool::call(&MoveText, a, self.cx.clone()).await
        } else {
            let a = CopyTextArgs {
                src: src.into(),
                before,
                after,
                at,
                indent: indent.into(),
            };
            Tool::call(&CopyText, a, self.cx.clone()).await
        }
    }
    async fn edit(&self, name: &str, range: &str, new: &str) -> anyhow::Result<ToolOutput> {
        let a = EditArgs {
            path: self.dir.join(name),
            old: None,
            range: Some(range.into()),
            new: Some(new.into()),
            all: false,
            edits: None,
        };
        Tool::call(&Edit, a, self.cx.clone()).await
    }
    async fn edits(&self, name: &str, ops: Vec<EditOp>) -> anyhow::Result<ToolOutput> {
        let a = EditArgs {
            path: self.dir.join(name),
            old: None,
            range: None,
            new: None,
            all: false,
            edits: Some(ops),
        };
        Tool::call(&Edit, a, self.cx.clone()).await
    }
    async fn indent(
        &self,
        name: &str,
        range: &str,
        by: Option<i64>,
        m: Option<&str>,
    ) -> anyhow::Result<ToolOutput> {
        let a = IndentArgs {
            path: self.dir.join(name),
            range: range.into(),
            by,
            r#match: m.map(str::to_owned),
        };
        Tool::call(&Indent, a, self.cx.clone()).await
    }
}

fn range_op(range: &str, new: &str) -> EditOp {
    EditOp {
        old: None,
        range: Some(range.into()),
        new: new.into(),
        all: None,
    }
}

#[tokio::test]
async fn read_header_carries_the_snapshot_id_but_not_over_4_mib() {
    let t = t();
    let p = t.put("a.txt", "x\ny\n");
    let out = t.read("a.txt").await;
    let id = crate::textref::snapshot_id(b"x\ny\n");
    let head = format!(
        "[read_file {}: 2 lines, 4 bytes, @{id}; showing lines 1–2]",
        p.display()
    );
    assert_eq!(out.lines().next().unwrap(), head);
    // The write refusal still recognises a pasted header.
    assert!(crate::fs::refuse_read_output(&out).is_err());
    // Recorded: a range edit against it works without another read.
    t.edit("a.txt", &format!("L2@{id}"), "Y\n").await.unwrap();
    assert_eq!(t.get("a.txt"), "x\nY\n");

    let big = "z\n".repeat(crate::textref::MAX_SNAPSHOT_BYTES / 2 + 1);
    t.put("big.txt", &big);
    let out = t.read("big.txt").await;
    assert!(
        !out.lines().next().unwrap().contains(", @"),
        "{}",
        &out[..200]
    );
    let e = t
        .edit(
            "big.txt",
            &format!("L1@{}", crate::textref::snapshot_id(big.as_bytes())),
            "",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("over 4 MiB"), "{e}");
}

#[tokio::test]
async fn line_refs_follow_own_edits_and_refuse_changed_lines_with_context() {
    let t = t();
    let text: String = (1..=10).map(|i| format!("l{i}\n")).collect();
    t.put("f.txt", &text);
    let id0 = t.id("f.txt").await;
    // Two lines inserted above, one changed below: L5-L6 still mean l5, l6.
    t.edits(
        "f.txt",
        vec![
            range_op(&format!("L1@{id0}"), "top\ntop2\nl1\n"),
            range_op(&format!("L9@{id0}"), "nine\n"),
        ],
    )
    .await
    .unwrap();
    let out = t
        .edit("f.txt", &format!("L5-L6@{id0}"), "X\n")
        .await
        .unwrap();
    assert!(out.content.starts_with("replaced "), "{}", out.content);
    assert_eq!(
        t.get("f.txt"),
        "top\ntop2\nl1\nl2\nl3\nl4\nX\nl7\nl8\nnine\nl10\n"
    );
    // A change to the named line itself is refused, with its text now.
    let e = t
        .edit("f.txt", &format!("L9@{id0}"), "")
        .await
        .unwrap_err()
        .to_string();
    let cur = crate::textref::snapshot_id(t.get("f.txt").as_bytes());
    assert!(
        e.contains("line 9 of") && e.contains(&format!("changed since @{id0}")),
        "{e}"
    );
    assert!(
        e.contains(&format!("is now @{cur}")) && e.contains("nine"),
        "{e}"
    );
    // The id in the refusal is usable at once.
    t.edit("f.txt", &format!("L10@{cur}"), "").await.unwrap();
    assert!(!t.get("f.txt").contains("nine"));
    let e = t
        .edit("f.txt", "L1@deadbeef", "")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("unknown snapshot @deadbeef") && e.contains("read the file again"),
        "{e}"
    );
}

#[tokio::test]
async fn edit_range_replaces_deletes_and_refuses_overlap_and_mixing() {
    let t = t();
    t.put("f.txt", "a\nb\nc\nd\ne\nf");
    let id = t.id("f.txt").await;
    let out = t.edit("f.txt", &format!("L2@{id}"), "B").await.unwrap();
    let p = t.dir.join("f.txt");
    let now = crate::textref::snapshot_id(t.get("f.txt").as_bytes());
    assert_eq!(
        out.content,
        format!(
            "replaced {0}:2 (1 line) with 1 line · {0} @{now}",
            p.display()
        )
    );
    let s = out.structured.unwrap();
    assert_eq!(s["op"], "edit");
    assert_eq!(s["files"][0]["snapshot"], now.as_str());
    assert!(s["files"][0]["diff"].as_str().unwrap().contains("-b\n+B\n"));
    t.edit("f.txt", &format!("L3-L4@{id}"), "").await.unwrap();
    assert_eq!(t.get("f.txt"), "a\nB\ne\nf");
    // The end of a file without a final newline keeps having none.
    let id = t.id("f.txt").await;
    t.edit("f.txt", &format!("L4@{id}"), "F").await.unwrap();
    assert_eq!(t.get("f.txt"), "a\nB\ne\nF");
    let id = t.id("f.txt").await;
    // Several ranges against one read apply together.
    t.edits(
        "f.txt",
        vec![
            range_op(&format!("L4@{id}"), "4\n"),
            range_op(&format!("L1@{id}"), "1\n2\n"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(t.get("f.txt"), "1\n2\nB\ne\n4\n");
    let id = t.id("f.txt").await;
    let e = t
        .edits(
            "f.txt",
            vec![
                range_op(&format!("L1-L3@{id}"), ""),
                range_op(&format!("L3@{id}"), ""),
            ],
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("overlap"), "{e}");
    let mixed = vec![
        EditOp {
            old: Some("B".into()),
            range: None,
            new: "b".into(),
            all: None,
        },
        range_op(&format!("L1@{id}"), ""),
    ];
    let e = t.edits("f.txt", mixed).await.unwrap_err().to_string();
    assert!(e.contains("one kind"), "{e}");
    assert_eq!(t.get("f.txt"), "1\n2\nB\ne\n4\n");
}

#[tokio::test]
async fn edit_old_mode_and_write_file_end_with_a_usable_id_line() {
    let t = t();
    let p = t.put("f.txt", "one\ntwo\n");
    let a = EditArgs {
        path: p.clone(),
        old: Some("two".into()),
        new: Some("2".into()),
        range: None,
        all: false,
        edits: None,
    };
    let out = Tool::call(&Edit, a, t.cx.clone()).await.unwrap();
    let id = crate::textref::snapshot_id(b"one\n2\n");
    assert!(
        out.content
            .starts_with(&format!("edited {}: 1 replacement", p.display()))
    );
    assert!(out.content.contains("-two\n+2\n"), "{}", out.content);
    assert_eq!(
        out.content.lines().last().unwrap(),
        format!("{} @{id}", p.display())
    );
    assert_eq!(out.structured.unwrap()["snapshot"], id.as_str());
    let w = WriteFileArgs {
        path: p.clone(),
        content: "one\n2\nthree\n".into(),
    };
    let out = Tool::call(&WriteFile, w, t.cx.clone()).await.unwrap();
    let id = crate::textref::snapshot_id(b"one\n2\nthree\n");
    assert_eq!(
        out.content.lines().last().unwrap(),
        format!("{} @{id}", p.display())
    );
    // Chained without a read.
    t.edit("f.txt", &format!("L3@{id}"), "3\n").await.unwrap();
    assert_eq!(t.get("f.txt"), "one\n2\n3\n");
}

#[tokio::test]
async fn move_across_files_and_within_one_both_ways() {
    let t = t();
    let a = t.put("a.txt", "1\n2\n3\n");
    let b = t.put("b.txt", "x\ny\n");
    let (ia, ib) = (t.id("a.txt").await, t.id("b.txt").await);
    let out = t
        .paste(
            true,
            &t.r("a.txt", &format!("L2@{ia}")),
            ("after", &t.r("b.txt", &format!("L1@{ib}"))),
            "keep",
        )
        .await
        .unwrap();
    assert_eq!(
        (t.get("a.txt"), t.get("b.txt")),
        ("1\n3\n".into(), "x\n2\ny\n".into())
    );
    let c = &out.content;
    assert!(
        c.starts_with(&format!(
            "moved 1 line {}:2 -> {}:2 (indent kept) · {} @",
            a.display(),
            b.display(),
            a.display()
        )),
        "{c}"
    );
    assert!(
        c.ends_with(&format!("\n  {0}:1  x\n  {0}:3  y", b.display())),
        "{c}"
    );
    assert_eq!(receipt_id(c, &b), crate::textref::snapshot_id(b"x\n2\ny\n"));
    assert_eq!(
        out.structured.unwrap()["files"].as_array().unwrap().len(),
        2
    );

    t.put("f.txt", "a\nb\nc\nd\n");
    let id = t.id("f.txt").await;
    t.paste(
        true,
        &t.r("f.txt", &format!("L1@{id}")),
        ("after", &t.r("f.txt", &format!("L3@{id}"))),
        "keep",
    )
    .await
    .unwrap();
    assert_eq!(t.get("f.txt"), "b\nc\na\nd\n");
    // The old id still names the same lines after the move.
    t.paste(
        true,
        &t.r("f.txt", &format!("L4@{id}")),
        ("before", &t.r("f.txt", &format!("L2@{id}"))),
        "keep",
    )
    .await
    .unwrap();
    assert_eq!(t.get("f.txt"), "d\nb\nc\na\n");
    let id = t.id("f.txt").await;
    let e = t
        .paste(
            true,
            &t.r("f.txt", &format!("L1-L3@{id}")),
            ("after", &t.r("f.txt", &format!("L2@{id}"))),
            "keep",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("inside the source"), "{e}");
    assert_eq!(t.get("f.txt"), "d\nb\nc\na\n");
}

#[tokio::test]
async fn paste_creates_files_and_keeps_crlf_bom_and_newline_state() {
    let t = t();
    t.put("a.txt", "1\n2\n");
    let ia = t.id("a.txt").await;
    let src = t.r("a.txt", &format!("L1-L2@{ia}"));
    t.paste(false, &src, ("at", &t.r("sub/new.txt", "end")), "match")
        .await
        .unwrap();
    assert_eq!(t.get("sub/new.txt"), "1\n2\n");
    assert_eq!(t.get("a.txt"), "1\n2\n", "copy leaves the source");

    t.put("crlf.txt", "x\r\ny\r\n");
    let ic = t.id("crlf.txt").await;
    t.paste(
        false,
        &src,
        ("after", &t.r("crlf.txt", &format!("L1@{ic}"))),
        "keep",
    )
    .await
    .unwrap();
    assert_eq!(t.get("crlf.txt"), "x\r\n1\r\n2\r\ny\r\n");

    // A last line without a newline gets one when it lands before a line.
    t.put("nonl.txt", "p\nq");
    let iq = t.id("nonl.txt").await;
    t.put("t.txt", "x\ny\n");
    let it = t.id("t.txt").await;
    t.paste(
        false,
        &t.r("nonl.txt", &format!("L2@{iq}")),
        ("before", &t.r("t.txt", &format!("L1@{it}"))),
        "keep",
    )
    .await
    .unwrap();
    assert_eq!(t.get("t.txt"), "q\nx\ny\n");
    // A target without a final newline gets one before an appended block.
    t.paste(false, &src, ("at", &t.r("nonl.txt", "end")), "keep")
        .await
        .unwrap();
    assert_eq!(t.get("nonl.txt"), "p\nq\n1\n2\n");

    t.put("bom.txt", "\u{feff}x\n");
    t.paste(false, &src, ("at", &t.r("bom.txt", "start")), "keep")
        .await
        .unwrap();
    assert_eq!(t.get("bom.txt"), "\u{feff}1\n2\nx\n");
}

#[cfg(unix)]
#[tokio::test]
async fn move_puts_the_source_back_when_the_target_write_fails() {
    use std::os::unix::fs::PermissionsExt as _;
    // Root ignores 0o555, so the failure cannot be forced.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let t = t();
    t.put("a.txt", "1\n2\n");
    std::fs::create_dir(t.dir.join("ro")).unwrap();
    t.put("ro/b.txt", "x\n");
    let (ia, ib) = (t.id("a.txt").await, t.id("ro/b.txt").await);
    let ro = t.dir.join("ro");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let res = t
        .paste(
            true,
            &t.r("a.txt", &format!("L1@{ia}")),
            ("after", &t.r("ro/b.txt", &format!("L1@{ib}"))),
            "keep",
        )
        .await;
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    let e = res.unwrap_err().to_string();
    assert!(
        e.contains("written first and restored, nothing changed"),
        "{e}"
    );
    assert_eq!(
        (t.get("a.txt"), t.get("ro/b.txt")),
        ("1\n2\n".into(), "x\n".into())
    );
}

#[tokio::test]
async fn indent_uses_detected_tab_and_editorconfig_units() {
    let t = t();
    t.put("s.txt", "fn a {\n  x\n\n  if y {\n    z\n  }\n}\n");
    let id = t.id("s.txt").await;
    let out = t
        .indent("s.txt", &format!("L2-L6@{id}"), Some(1), None)
        .await
        .unwrap();
    assert_eq!(
        t.get("s.txt"),
        "fn a {\n    x\n\n    if y {\n      z\n    }\n}\n"
    );
    let p = t.dir.join("s.txt");
    assert!(
        out.content.starts_with(&format!(
            "indented {0}:2-6 by +2 spaces (1 level) · {0} @",
            p.display()
        )),
        "{}",
        out.content
    );
    assert!(out.content.contains("no parser"), "{}", out.content);

    t.put("tab.txt", "a {\n\tb\n\t\tc\n  \t\n}\n");
    let id = t.id("tab.txt").await;
    let out = t
        .indent("tab.txt", &format!("L2-L4@{id}"), Some(-1), None)
        .await
        .unwrap();
    assert_eq!(
        t.get("tab.txt"),
        "a {\nb\n\tc\n\n}\n",
        "blank lines emptied"
    );
    assert!(
        out.content.contains("by -1 tab (1 level)"),
        "{}",
        out.content
    );

    std::fs::create_dir(t.dir.join("ec")).unwrap();
    std::fs::write(
        t.dir.join("ec/.editorconfig"),
        "[*]\nindent_style = space\nindent_size = 3\n",
    )
    .unwrap();
    t.put("ec/e.txt", "a\n\tb\n");
    let id = t.id("ec/e.txt").await;
    t.indent("ec/e.txt", &format!("L1-L2@{id}"), Some(1), None)
        .await
        .unwrap();
    // No tab_width: a numeric indent_size is the tab width too (§3).
    assert_eq!(
        t.get("ec/e.txt"),
        "   a\n      b\n",
        "tab measured as 3, rendered as spaces"
    );

    // Nothing to detect: 4.
    t.put("n.txt", "a\nb\n");
    let id = t.id("n.txt").await;
    t.indent("n.txt", &format!("L2@{id}"), Some(1), None)
        .await
        .unwrap();
    assert_eq!(t.get("n.txt"), "a\n    b\n");
}

/// 193 R3 #3: a failed syntax hook refuses the shift with its own message
/// (not the no-parser note) and writes nothing; no parser keeps the note.
#[tokio::test]
async fn failed_syntax_hook_refuses_a_shift() {
    let t = t();
    let body = "a {\n  b\n}\n";
    t.put("x.boom", body);
    let id = t.id("x.boom").await;
    let e = t
        .indent("x.boom", &format!("L2@{id}"), Some(1), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("the syntax parser failed on"), "{e}");
    assert!(!e.contains("no parser"), "{e}");
    assert_eq!(t.get("x.boom"), body);

    t.put("x.txt", body);
    let id = t.id("x.txt").await;
    let out = t
        .indent("x.txt", &format!("L2@{id}"), Some(1), None)
        .await
        .unwrap();
    assert!(out.content.contains("no parser"), "{}", out.content);
}

/// 193 R3 #8: an ACP child seated as `acp_session` seats it (the parent's
/// seat inherited, then the child's own cwd): a relative path given to its
/// tools resolves in the child's cwd, not the parent's.
#[tokio::test]
async fn an_acp_childs_relative_path_resolves_in_its_own_cwd() {
    let t = t();
    std::fs::create_dir_all(t.dir.join("parent")).unwrap();
    std::fs::create_dir_all(t.dir.join("child")).unwrap();
    t.put("parent/a.txt", "x\n");
    t.put("child/a.txt", "x\n");
    let (p, c) = (crate::SessionId::new(), t.cx.session);
    crate::seat::set_cwd(p, Some(t.dir.join("parent")));
    crate::seat::set_write_root(p, Some(t.dir.clone()));
    crate::seat::inherit(c, p);
    crate::seat::set_cwd(c, Some(t.dir.join("child")));
    let read = Tool::call(
        &ReadFile,
        ReadFileArgs {
            path: "a.txt".into(),
            offset: None,
            limit: None,
            tail: None,
            ranges: None,
            numbers: true,
            outline: false,
        },
        t.cx.clone(),
    )
    .await
    .unwrap()
    .content;
    let args = IndentArgs {
        path: "a.txt".into(),
        range: format!("L1@{}", id_of(&read)),
        by: Some(1),
        r#match: None,
    };
    Tool::call(&Indent, args, t.cx.clone()).await.unwrap();
    crate::seat::forget(c);
    crate::seat::forget(p);
    assert_eq!(t.get("child/a.txt"), "    x\n");
    assert_eq!(t.get("parent/a.txt"), "x\n");
}

#[tokio::test]
async fn indent_match_and_negative_refusal() {
    let t = t();
    t.put("m.txt", "      x\ny\n  z\n");
    let id = t.id("m.txt").await;
    t.indent(
        "m.txt",
        &format!("L2-L3@{id}"),
        None,
        Some(&format!("L1@{id}")),
    )
    .await
    .unwrap();
    assert_eq!(t.get("m.txt"), "      x\n      y\n        z\n");
    let id = t.id("m.txt").await;
    let e = t
        .indent("m.txt", &format!("L2-L3@{id}"), Some(-4), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("cut into") && e.contains("m.txt:2") && e.contains("`y`"),
        "{e}"
    );
    assert!(
        t.indent("m.txt", &format!("L2@{id}"), None, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn paste_match_follows_each_rule() {
    let t = t();
    t.put("src.txt", "zzz\n");
    let is = t.id("src.txt").await;
    let src = t.r("src.txt", &format!("L1@{is}"));
    // Rule 3: prev opens a block.
    t.put("o.txt", "fn f() {  // c\n}\n");
    let id = t.id("o.txt").await;
    let out = t
        .paste(
            false,
            &src,
            ("after", &t.r("o.txt", &format!("L1@{id}"))),
            "match",
        )
        .await
        .unwrap();
    assert_eq!(t.get("o.txt"), "fn f() {  // c\n    zzz\n}\n");
    assert!(out.content.contains("(indent +4)"), "{}", out.content);
    // Next at least as deep as prev.
    t.put("n.txt", "a\n    b\n");
    let id = t.id("n.txt").await;
    t.paste(
        false,
        &src,
        ("before", &t.r("n.txt", &format!("L2@{id}"))),
        "match",
    )
    .await
    .unwrap();
    assert_eq!(t.get("n.txt"), "a\n    zzz\n    b\n");
    // Next shallower: prev's depth.
    t.put("p.txt", "    a\nb\n");
    let id = t.id("p.txt").await;
    t.paste(
        false,
        &src,
        ("before", &t.r("p.txt", &format!("L2@{id}"))),
        "match",
    )
    .await
    .unwrap();
    assert_eq!(t.get("p.txt"), "    a\n    zzz\nb\n");
    // Rule 2: start/end is column 0.
    t.put("e.txt", "    a\n");
    let ip = t.id("p.txt").await;
    let deep = t.r("p.txt", &format!("L2@{ip}"));
    t.paste(false, &deep, ("at", &t.r("e.txt", "end")), "match")
        .await
        .unwrap();
    assert_eq!(t.get("e.txt"), "    a\nzzz\n");
    // Rule 1: a symbol target makes the block its sibling.
    t.put("x.fk", "type T\n    fn a\n        body\n");
    t.put("blk.txt", "fn b\n    body2\n");
    let ib = t.id("blk.txt").await;
    t.paste(
        false,
        &t.r("blk.txt", &format!("L1-L2@{ib}")),
        ("after", &t.r("x.fk", "T.a")),
        "match",
    )
    .await
    .unwrap();
    assert_eq!(
        t.get("x.fk"),
        "type T\n    fn a\n        body\n    fn b\n        body2\n"
    );
    // "+N" shifts from the source's own depth.
    t.put("k.txt", "q\n");
    t.paste(false, &src, ("at", &t.r("k.txt", "end")), "+2")
        .await
        .unwrap();
    assert_eq!(t.get("k.txt"), "q\n        zzz\n");
}

#[tokio::test]
async fn symbol_refs_widen_and_string_interiors_are_kept() {
    let t = t();
    t.put("f.fk", "/// doc\nfn a\n    x\nfn b\n    y\n");
    let out = t
        .paste(
            true,
            &t.r("f.fk", "b"),
            ("before", &t.r("f.fk", "a")),
            "match",
        )
        .await
        .unwrap();
    assert_eq!(
        t.get("f.fk"),
        "fn b\n    y\n/// doc\nfn a\n    x\n",
        "{}",
        out.content
    );
    t.edit("f.fk", "a", "").await.unwrap();
    assert_eq!(t.get("f.fk"), "fn b\n    y\n");
    let e = t.edit("f.fk", "nope", "").await.unwrap_err().to_string();
    assert!(e.contains("no symbol `nope`"), "{e}");
    t.put("plain.txt", "fn a\n");
    let e = t.edit("plain.txt", "a", "").await.unwrap_err().to_string();
    assert!(e.contains("need a parser"), "{e}");

    t.put("s.fk", "fn s\n    t = \"\"\"\n  keep\n    \"\"\"\n    u\n");
    let out = t.indent("s.fk", "s", Some(1), None).await.unwrap();
    assert_eq!(
        t.get("s.fk"),
        "    fn s\n        t = \"\"\"\n  keep\n    \"\"\"\n        u\n"
    );
    assert!(
        out.content
            .ends_with("· 2 lines inside a multi-line string kept"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn clips_store_replace_paste_and_name_what_exists() {
    let t = t();
    t.put("a.txt", "one\ntwo\n");
    t.put("b.txt", "");
    let id = t.id("a.txt").await;
    let clip = |src: String| ClipArgs {
        name: "x".into(),
        src,
    };
    let out = Tool::call(
        &ClipTool,
        clip(t.r("a.txt", &format!("L1@{id}"))),
        t.cx.clone(),
    )
    .await
    .unwrap();
    assert!(
        out.content.starts_with("clipped 1 line "),
        "{}",
        out.content
    );
    // Taken as it is now: a later change to the file does not reach it.
    t.put("a.txt", "ONE\ntwo\n");
    t.paste(false, "clip:x", ("at", &t.r("b.txt", "end")), "keep")
        .await
        .unwrap();
    assert_eq!(t.get("b.txt"), "one\n");
    let id = t.id("a.txt").await;
    Tool::call(
        &ClipTool,
        clip(t.r("a.txt", &format!("L2@{id}"))),
        t.cx.clone(),
    )
    .await
    .unwrap();
    t.paste(false, "clip:x", ("at", &t.r("b.txt", "end")), "keep")
        .await
        .unwrap();
    assert_eq!(t.get("b.txt"), "one\ntwo\n");
    let e = t
        .paste(false, "clip:nope", ("at", &t.r("b.txt", "end")), "keep")
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("no clip `nope`; clips: x"), "{e}");
    assert!(
        t.paste(true, "clip:x", ("at", &t.r("b.txt", "end")), "keep")
            .await
            .is_err()
    );

    let s = crate::SessionId::new();
    let c = |n: usize| Clip {
        lines: vec!["y".repeat(n)],
        final_nl: true,
        strings: None,
    };
    for i in 0..crate::textref::CLIP_COUNT {
        crate::textref::put_clip(s, &format!("c{i}"), c(1)).unwrap();
    }
    assert!(
        crate::textref::put_clip(s, "c0", c(1)).is_ok(),
        "replacing is fine at the cap"
    );
    let e = crate::textref::put_clip(s, "more", c(1))
        .unwrap_err()
        .to_string();
    assert!(e.contains("64 clips"), "{e}");
    let e = crate::textref::put_clip(s, "c1", c(crate::textref::CLIP_MAX))
        .unwrap_err()
        .to_string();
    assert!(e.contains("at most 1 MiB"), "{e}");
    let s = crate::SessionId::new();
    for i in 0..16 {
        crate::textref::put_clip(s, &format!("m{i}"), c(crate::textref::CLIP_MAX - 1)).unwrap();
    }
    let e = crate::textref::put_clip(s, "m16", c(10))
        .unwrap_err()
        .to_string();
    assert!(e.contains("16 MiB"), "{e}");
}

/// Snapshots and clips belong to the session that made them: another
/// session's id is unknown even while the file is unchanged.
#[tokio::test]
async fn snapshots_and_clips_do_not_cross_sessions() {
    let t = t();
    t.put("f.txt", "a\nb\n");
    let id = t.id("f.txt").await;
    let mut other = t.cx.clone();
    other.session = crate::SessionId::new();
    let a = EditArgs {
        path: t.dir.join("f.txt"),
        old: None,
        range: Some(format!("L1@{id}")),
        new: Some("A\n".into()),
        all: false,
        edits: None,
    };
    let e = Tool::call(&Edit, a, other.clone())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains(&format!("unknown snapshot @{id}")), "{e}");
    let clip = ClipArgs {
        name: "x".into(),
        src: t.r("f.txt", &format!("L1@{id}")),
    };
    Tool::call(&ClipTool, clip, t.cx.clone()).await.unwrap();
    let paste = CopyTextArgs {
        src: "clip:x".into(),
        before: None,
        after: None,
        at: Some(t.r("f.txt", "end")),
        indent: "keep".into(),
    };
    let e = Tool::call(&CopyText, paste, other)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("this session has no clips"), "{e}");
    assert_eq!(t.get("f.txt"), "a\nb\n");
}

#[tokio::test]
async fn an_insertion_inside_the_named_lines_refuses() {
    let t = t();
    t.put("f.txt", "1\n2\n3\n4\n5\n");
    let id = t.id("f.txt").await;
    t.put("f.txt", "1\n2\n3\nnew\n4\n5\n");
    let e = t
        .edit("f.txt", &format!("L3-L4@{id}"), "")
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("lines 3-4 of") && e.contains("new"), "{e}");
    // Outside the insertion still maps.
    t.edit("f.txt", &format!("L5@{id}"), "FIVE\n")
        .await
        .unwrap();
    assert_eq!(t.get("f.txt"), "1\n2\n3\nnew\n4\nFIVE\n");
}

#[tokio::test]
async fn refusals_cap_context_at_40_lines_and_say_when_nothing_is_left() {
    let t = t();
    let text: String = (1..=60).map(|i| format!("l{i}\n")).collect();
    t.put("f.txt", &text);
    let id = t.id("f.txt").await;
    let changed: String = (1..=60)
        .map(|i| {
            if (5..=55).contains(&i) {
                format!("X{i}\n")
            } else {
                format!("l{i}\n")
            }
        })
        .collect();
    t.put("f.txt", &changed);
    let e = t
        .edit("f.txt", &format!("L5-L55@{id}"), "")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("[11 more lines]") && e.contains("X44") && !e.contains("X45"),
        "{e}"
    );
    // The last lines deleted: nothing to show.
    t.put("g.txt", "a\nb\nc\nd\n");
    let id = t.id("g.txt").await;
    t.put("g.txt", "a\nb\n");
    let e = t
        .edit("g.txt", &format!("L3-L4@{id}"), "")
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("(no lines there now)"), "{e}");
    // A long line is cut in the refusal.
    t.put("h.txt", "a\nb\n");
    let id = t.id("h.txt").await;
    t.put("h.txt", &format!("a\n{}\n", "z".repeat(500)));
    let e = t
        .edit("h.txt", &format!("L2@{id}"), "")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains(" …[+300 chars]") && !e.contains(&"z".repeat(201)),
        "{e}"
    );
}

#[tokio::test]
async fn mapping_works_on_crlf_bom_and_no_final_newline_files() {
    let t = t();
    for (name, before, after) in [
        ("crlf.txt", "a\r\nb\r\nc\r\n", "top\r\na\r\nb\r\nC\r\n"),
        ("bom.txt", "\u{feff}a\nb\nc\n", "\u{feff}top\na\nb\nC\n"),
        ("nonl.txt", "a\nb\nc", "top\na\nb\nC"),
    ] {
        t.put(name, before);
        let id = t.id(name).await;
        t.put(name, &before.replacen('a', "top\na", 1));
        t.edit(name, &format!("L3@{id}"), "C").await.unwrap();
        assert_eq!(t.get(name), after, "{name}");
    }
}

#[tokio::test]
async fn edit_range_argument_errors_and_symbol_delete_at_eof() {
    let t = t();
    t.put("f.txt", "a\nb\n");
    let id = t.id("f.txt").await;
    let args = |old: Option<&str>, range: Option<&str>, new: &str, all: bool| EditArgs {
        path: t.dir.join("f.txt"),
        old: old.map(str::to_owned),
        range: range.map(str::to_owned),
        new: Some(new.into()),
        all,
        edits: None,
    };
    let call = |a: EditArgs| Tool::call(&Edit, a, t.cx.clone());
    let l1 = format!("L1@{id}");
    let e = call(args(Some("a"), Some(&l1), "x", false))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("`old` or `range`, not both"), "{e}");
    let e = call(args(None, Some(&l1), "x", true))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("`all` applies to `old` edits only"), "{e}");
    for edge in ["start", "end"] {
        let e = call(args(None, Some(edge), "x", false))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("not start/end"), "{e}");
    }
    let header = format!("[read_file f.txt: 2 lines, 4 bytes, @{id}; showing lines 1–2]\n");
    let e = call(args(None, Some(&l1), &header, false))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("looks like read_file output"), "{e}");
    assert_eq!(t.get("f.txt"), "a\nb\n");

    t.put("s.fk", "fn a\n    x\nfn b\n    y");
    t.edit("s.fk", "b", "").await.unwrap();
    assert_eq!(t.get("s.fk"), "fn a\n    x");
}

#[tokio::test]
async fn match_rules_cover_openers_edges_and_outdent() {
    let t = t();
    t.put("src.txt", "zzz\n");
    let is = t.id("src.txt").await;
    let src = t.r("src.txt", &format!("L1@{is}"));
    let after = |name: &str, id: &str, n: usize| t.r(name, &format!("L{n}@{id}"));
    for (opener, want) in [
        ("call(", "call(\n    zzz\n)\n"),
        ("list = [", "list = [\n    zzz\n)\n"),
        ("def f():  # c", "def f():  # c\n    zzz\n)\n"),
    ] {
        t.put("o.txt", &format!("{opener}\n)\n"));
        let id = t.id("o.txt").await;
        t.paste(false, &src, ("after", &after("o.txt", &id, 1)), "match")
            .await
            .unwrap();
        assert_eq!(t.get("o.txt"), want, "{opener}");
    }
    // No line above: the next line's depth.
    t.put("n.txt", "    a\n");
    let id = t.id("n.txt").await;
    t.paste(false, &src, ("before", &after("n.txt", &id, 1)), "match")
        .await
        .unwrap();
    assert_eq!(t.get("n.txt"), "    zzz\n    a\n");
    // Only blank lines around: column 0.
    t.put("b.txt", "\n\n");
    let id = t.id("b.txt").await;
    t.paste(
        false,
        &t.r("n.txt", &format!("L1@{}", t.id("n.txt").await)),
        ("before", &after("b.txt", &id, 2)),
        "match",
    )
    .await
    .unwrap();
    assert_eq!(t.get("b.txt"), "\nzzz\n\n");
    // "-1" on a paste outdents from the source's own depth.
    t.put("d.txt", "q\n");
    let id = t.id("n.txt").await;
    t.paste(
        false,
        &t.r("n.txt", &format!("L2@{id}")),
        ("at", &t.r("d.txt", "end")),
        "-1",
    )
    .await
    .unwrap();
    assert_eq!(t.get("d.txt"), "q\na\n");
    // Bounds: too many levels, too wide.
    let e = t
        .paste(false, &src, ("at", &t.r("d.txt", "end")), "+65")
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("at most 64"), "{e}");
    let e = t
        .indent("d.txt", "L1@00000000", Some(1_000_000_000), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("at most 64"), "{e}");
    std::fs::write(
        t.dir.join(".editorconfig"),
        "root = true\n[w.txt]\nindent_size = 1000\n",
    )
    .unwrap();
    t.put("w.txt", "a\n");
    let id = t.id("w.txt").await;
    let e = t
        .indent("w.txt", &format!("L1@{id}"), Some(2), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("limit is 1024"), "{e}");
}

#[tokio::test]
async fn string_flags_survive_a_clip_and_non_utf8_is_refused() {
    let t = t();
    t.put("s.fk", "fn s\n    t = \"\"\"\n  keep\n    \"\"\"\n");
    let clip = ClipArgs {
        name: "s".into(),
        src: t.r("s.fk", "s"),
    };
    Tool::call(&ClipTool, clip, t.cx.clone()).await.unwrap();
    t.put("out.txt", "");
    let out = t
        .paste(false, "clip:s", ("at", &t.r("out.txt", "end")), "+1")
        .await
        .unwrap();
    assert_eq!(
        t.get("out.txt"),
        "    fn s\n        t = \"\"\"\n  keep\n    \"\"\"\n"
    );
    assert!(
        out.content
            .contains("2 lines inside a multi-line string kept"),
        "{}",
        out.content
    );

    std::fs::write(t.dir.join("bin.txt"), [0xff, 0xfe, b'\n']).unwrap();
    let e = t
        .indent("bin.txt", "L1@00000000", Some(1), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("not UTF-8"), "{e}");
}
