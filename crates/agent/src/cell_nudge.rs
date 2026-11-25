//! 220 cell nudge: a short shell/python fence the reply asks the user to run, written without
//! a `#|` header, is promoted to a user cell in place. This module is the pure half: the fence
//! scanner, the detector ([`detect`]), the repair side call's prompt ([`REPAIR_SYSTEM`],
//! [`repair_user`]), the acceptance checks of its answer ([`accept`]) and the per-session
//! attempt book ([`NudgeBook`]). The session actor (`loop_`, `PromoteFence`) runs the call and
//! the compare-and-swap replace.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::LazyLock;
use std::time::Duration;

use genai::chat::{ContentPart, MessageContent};
use regex::Regex;
use surface::cell::{self, Lang};

/// The repair call's wall-clock budget.
pub const REPAIR_TIMEOUT: Duration = Duration::from_secs(20);
/// The repair call's reply cap: one short cell (the call carries no reasoning effort).
pub const REPAIR_CAP: u32 = 800;
/// Most non-blank body lines a fence the detector fires on may have.
const MAX_LINES: usize = 5;
/// Prose window before a fence searched for a run-me signal.
const SIGNAL_BEFORE: usize = 300;
/// Prose window after a fence searched for a run-me signal.
const SIGNAL_AFTER: usize = 200;
/// Reply prose the repair call sees before the fence.
const CONTEXT_BEFORE: usize = 1000;
/// Reply prose the repair call sees after the fence.
const CONTEXT_AFTER: usize = 500;
/// The only header keys a repaired cell may carry: the rest (`env`, `python`, `review`,
/// `private`, `after`, `on`) change what or how it runs, or what the agent is told.
const ALLOWED_KEYS: [&str; 6] = ["cwd", "label", "danger", "expect", "name", "timeout"];

/// One closed plain fence of a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlainFence {
    /// 0-based index among the reply's plain fences, in document order.
    pub ordinal: u32,
    /// The info string's first word, as written.
    pub lang: String,
    /// Everything between the fence lines, trailing newline included, dedented by the opening
    /// line's indent (a fence in a list item).
    pub body: String,
    /// Opening line start .. closing line end (its newline excluded).
    pub range: Range<usize>,
    /// Leading spaces of the opening line (0–3): a repaired cell is re-indented by it.
    pub indent: usize,
}

/// What [`detect`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Promote the plain fence `ordinal`; `reason` names the signal (`"command"` or `"prose"`).
    Nudge {
        /// Plain-fence ordinal.
        ordinal: u32,
        /// Which signal fired.
        reason: &'static str,
    },
    /// Nothing to promote.
    Skip,
}

/// What the repair answer amounts to ([`accept`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Repair {
    /// A valid cell fence (its text, no trailing newline) that replaces the original fence.
    Cell(String),
    /// The model answered `OK` (or nothing): the fence stays a snippet.
    Declined,
    /// The answer failed a check; why.
    Rejected(String),
}

/// 220: the automatic nudges of one session — how many were spent and the digest of every
/// fence tried, so a fence is tried once. An attempt that never got the model's verdict
/// (timeout, provider error, a turn started) is given back. In memory only.
#[derive(Debug, Default)]
pub struct NudgeBook {
    count: u8,
    tried: BTreeSet<String>,
}

impl NudgeBook {
    /// Nudges spent.
    pub fn count(&self) -> u8 {
        self.count
    }

    /// Spend one on the fence `digest`: `false` (nothing spent) when it was tried before or
    /// `max` are spent.
    pub fn take(&mut self, digest: &str, max: u8) -> bool {
        if self.count >= max || self.tried.contains(digest) {
            return false;
        }
        self.tried.insert(digest.to_owned());
        self.count += 1;
        true
    }

    /// Undo a [`NudgeBook::take`] of `digest` whose attempt ended without a verdict.
    pub fn give_back(&mut self, digest: &str) {
        if self.tried.remove(digest) {
            self.count = self.count.saturating_sub(1);
        }
    }
}

/// The reply text the nudge works on: the message's text parts joined, nothing else (binary
/// tags and tool output are not reply prose). The UI's ordinals count over the same text.
pub fn reply_text(content: &MessageContent) -> String {
    content
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

/// The runnable language of an info string (`surface::cell::lang_of`), JS/TS excluded: those
/// never promote (220 Vocabulary).
pub fn runnable(info: &str) -> Option<Lang> {
    cell::lang_of(info).filter(|l| !matches!(l, Lang::Javascript | Lang::Typescript))
}

/// A closed fence: trimmed info string, dedented body (trailing newline included), line range,
/// opening indent.
struct Fence<'a> {
    info: &'a str,
    body: String,
    range: Range<usize>,
    indent: usize,
}

/// Every closed fence of `text`, the CommonMark way (as `surface::fences`, which is
/// crate-private): an opening line of three or more `` ` `` or `~` (indent ≤ 3, a backtick
/// fence's info has no backtick), closed by a line of at least as many of the same character
/// and nothing else; body lines lose up to the opener's indent in spaces. Fences inside a
/// fence are content; an unclosed fence runs to the end and is dropped.
fn fences(text: &str) -> Vec<Fence<'_>> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        lines.push((offset, line));
        offset += line.len();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (start, line) = lines[i];
        let Some((marker, len, info)) = opening(line) else {
            i += 1;
            continue;
        };
        let Some(j) = (i + 1..lines.len()).find(|&j| closing(lines[j].1, marker, len)) else {
            break;
        };
        let indent = line.bytes().take_while(|b| *b == b' ').count();
        let body = lines[i + 1..j]
            .iter()
            .map(|(_, l)| {
                let n = l.bytes().take(indent).take_while(|b| *b == b' ').count();
                &l[n..]
            })
            .collect();
        let (cstart, cline) = lines[j];
        out.push(Fence {
            info,
            body,
            range: start..cstart + cline.trim_end_matches(['\n', '\r']).len(),
            indent,
        });
        i = j + 1;
    }
    out
}

/// The line with up to three leading spaces removed; `None` if indented further.
fn unindented(line: &str) -> Option<&str> {
    let n = line.bytes().take_while(|b| *b == b' ').count();
    (n <= 3).then_some(&line[n..])
}

/// `(marker, run length, trimmed info string)` if `line` opens a fence.
fn opening(line: &str) -> Option<(u8, usize, &str)> {
    let line = unindented(line)?;
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = line.bytes().take_while(|b| *b == marker).count();
    let rest = &line[len..];
    if len < 3 || (marker == b'`' && rest.contains('`')) {
        return None;
    }
    Some((marker, len, rest.trim()))
}

/// Whether `line` closes a fence opened by `len` × `marker`.
fn closing(line: &str, marker: u8, len: usize) -> bool {
    unindented(line).is_some_and(|l| {
        let n = l.bytes().take_while(|b| *b == marker).count();
        n >= len && l[n..].trim().is_empty()
    })
}

/// A plain fence: runnable (JS/TS excluded) and no cell header.
fn is_plain(f: &Fence<'_>) -> bool {
    runnable(f.info).is_some() && cell::parse(f.info, &f.body).is_none()
}

/// Every closed plain fence of `text`: a runnable language (JS/TS excluded) and no cell
/// header (`surface::cell::parse` is `None`), numbered in document order.
pub fn plain_fences(text: &str) -> Vec<PlainFence> {
    fences(text)
        .into_iter()
        .filter(is_plain)
        .enumerate()
        .map(|(i, f)| PlainFence {
            ordinal: i as u32,
            lang: f.info.split_whitespace().next().unwrap_or_default().to_owned(),
            body: f.body,
            range: f.range,
            indent: f.indent,
        })
        .collect()
}

/// A fence body as compared by [`pick`]: trailing whitespace off each line, trailing
/// newlines off the end.
fn same_body(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end_matches('\n')
            .to_owned()
    };
    norm(a) == norm(b)
}

/// The plain fence of `text` a caller means (220 build notes, ordinal drift guard): without
/// `body`, the fence at `ordinal`; with it, the fence at `ordinal` when its body matches, else
/// the first plain fence whose body matches — the ordinal only breaks ties between identical
/// bodies, so a client whose scanner numbers fences differently never rewrites the wrong one.
pub fn pick(text: &str, ordinal: u32, body: Option<&str>) -> Option<PlainFence> {
    let mut all = plain_fences(text);
    let at = match body {
        None => all.iter().position(|f| f.ordinal == ordinal)?,
        Some(body) => all
            .iter()
            .position(|f| f.ordinal == ordinal && same_body(&f.body, body))
            .or_else(|| all.iter().position(|f| same_body(&f.body, body)))?,
    };
    Some(all.swap_remove(at))
}

/// Prose asking the user to run the fence, anywhere in the windows around it.
static PROSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)run (?:this|it|the following|that) (?:yourself|on|in|from)|tell me when (?:it['’]s|it is)? ?done|paste (?:the|its) output|once (?:you['’]ve|you have) run|you(?:['’]ll| will) need to run|in your terminal",
    )
    .expect("prose re")
});
/// The clause right before the fence is a bare run instruction (`Run this:`, `Now run:`).
static RUN_COLON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:now |then |next,? )?run(?: (?:this|these|the following|it)(?: commands?)?)?:$")
        .expect("run colon re")
});
/// The clause right before the fence opens with an imperative run (`Run the reload …`).
static LEAD_IN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:now |then |next,? )?run\b").expect("lead-in re"));
/// The clause right before the fence introduces an illustration.
static SUPPRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)for example|e\.g\.|the syntax is|you could|something like|such as|equivalent to|in general",
    )
    .expect("suppress re")
});
/// `<word>`: a placeholder. A heredoc (`<<EOF`) or a redirection (`< file`) never matches.
static ANGLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^<])<[A-Za-z][A-Za-z0-9_.-]*>").expect("angle re"));
static FILLER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)your_|changeme|xxx|<!--").expect("filler re"));
/// Commands that change state (privilege, services, clusters, packages).
const CHANGING: [&str; 9] = [
    "sudo ",
    "systemctl ",
    "kubectl apply",
    "kubectl delete",
    "helm upgrade",
    "terraform apply",
    "brew install",
    "apt install",
    "dnf install",
];

/// A command only the user's own shell runs: one that changes state, or an interactive login.
fn user_command(body: &str) -> bool {
    changes_state(body) || body.contains("ssh -t")
}

/// The fence changes state: a repaired cell must carry `#| danger`.
pub fn changes_state(body: &str) -> bool {
    CHANGING.iter().any(|c| body.contains(c))
}

/// The last `n` chars of `text`.
fn last_chars(text: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    match text.char_indices().rev().nth(n - 1) {
        Some((i, _)) => &text[i..],
        None => text,
    }
}

/// The first `n` chars of `text`.
fn first_chars(text: &str, n: usize) -> &str {
    let to = text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    &text[..to]
}

/// The clause right before a fence: the last line of `lead`, from its last sentence start
/// (`.`, `!` or `?`, whitespace, then a capital — so `e.g. with` stays one clause), trimmed.
fn last_clause(lead: &str) -> &str {
    let t = lead.trim_end();
    let line = &t[t.rfind('\n').map_or(0, |i| i + 1)..];
    let head = line.trim_end_matches(['.', '!', '?', ':']);
    let b = head.as_bytes();
    let start = (0..b.len())
        .rev()
        .find(|&i| {
            matches!(b[i], b'.' | b'!' | b'?')
                && b.get(i + 1).is_some_and(u8::is_ascii_whitespace)
                && head[i + 1..]
                    .trim_start()
                    .starts_with(|c: char| c.is_uppercase())
        })
        .map_or(0, |i| i + 1);
    line[start..].trim()
}

/// A placeholder the user must fill in first: such a fence is a template, not a command.
fn placeholder(body: &str) -> bool {
    ANGLE.is_match(body)
        || FILLER.is_match(body)
        || body.lines().any(|l| matches!(l.trim(), "..." | "…"))
}

/// 220: the first plain fence of `text` that is short (≤ 5 non-blank lines), has no
/// placeholder, whose introducing clause is no illustration ("for example", "you could", …),
/// and that carries a run-me signal: prose asking the user to run it (300 chars before, 200
/// after, the windows stopping at the neighbouring fences) or a bare `Run this:` right before
/// it (`"prose"`); a command only the user's shell runs (`sudo`, `ssh -t`, `brew install`, …)
/// counts only with such prose or an imperative `Run …` clause right before it (`"command"`).
pub fn detect(text: &str) -> Verdict {
    let all = fences(text);
    let mut ordinal = 0u32;
    for (i, f) in all.iter().enumerate() {
        if !is_plain(f) {
            continue;
        }
        let this = ordinal;
        ordinal += 1;
        let lead = &text[i.checked_sub(1).map_or(0, |p| all[p].range.end)..f.range.start];
        let trail = &text[f.range.end..all.get(i + 1).map_or(text.len(), |n| n.range.start)];
        let clause = last_clause(lead);
        if f.body.lines().filter(|l| !l.trim().is_empty()).count() > MAX_LINES
            || placeholder(&f.body)
            || SUPPRESS.is_match(clause)
        {
            continue;
        }
        let prose = PROSE.is_match(last_chars(lead, SIGNAL_BEFORE))
            || PROSE.is_match(first_chars(trail, SIGNAL_AFTER))
            || RUN_COLON.is_match(clause);
        let reason = if user_command(&f.body) && (prose || LEAD_IN.is_match(clause)) {
            "command"
        } else if prose {
            "prose"
        } else {
            continue;
        };
        return Verdict::Nudge {
            ordinal: this,
            reason,
        };
    }
    Verdict::Skip
}

/// The repair call's system prompt: the cell grammar of `cells::prompt` cut to one fence.
pub const REPAIR_SYSTEM: &str = "\
You turn one fenced block from an AI assistant's chat reply into a theuth user cell, or leave it alone.

A user cell is a fenced block in bash, zsh, sh, pwsh or python whose first body lines are `#| key: value` option lines (a bare `#| key` means true); the user runs it with one press. The header ends at the first other line; the code after it is what runs.

Answer with exactly OK (nothing else) when the block is not something the reply asks the user to run: an illustration, a fragment, a template with placeholders, sample output, or commands for another machine the user pastes elsewhere.

Otherwise answer with exactly one fenced block and nothing before or after it:
- the same language word as the original fence;
- `#| cwd:` always: an absolute path or ~/…; the session cwd given below unless the reply names another folder;
- when the code changes state (installs, restarts, reloads, applies, deletes, writes, sudo): `#| label:` naming the consequence in at most 40 characters (\"Reload alloy on Admin\") and a bare `#| danger`;
- `#| expect:` with text the output shows on success, only when the reply says what success looks like (repeatable, or a /regex/);
- then the original code exactly as written, except a leading `$ ` prompt removed from each line. Never add, drop, reorder or edit a command, a flag or a quote.
Only these options exist for you: cwd, label, danger, expect, and when the reply calls for them `#| name:` (slug of a-z, 0-9 and -, at most 48) and `#| timeout:` (30s, 10m, 2h). Write no other option.";

/// The repair call's user message: up to 1500 chars of the reply around the fence, the fence
/// verbatim, the session cwd.
pub fn repair_user(text: &str, fence: &PlainFence, cwd: &str) -> String {
    let shown = &text[fence.range.clone()];
    format!(
        "Session cwd: {cwd}\n\n<reply_excerpt>\n{}{shown}{}\n</reply_excerpt>\n\nThe fence to rewrite:\n{shown}",
        last_chars(&text[..fence.range.start], CONTEXT_BEFORE),
        first_chars(&text[fence.range.end..], CONTEXT_AFTER),
    )
}

/// Code as compared by [`accept`]: a leading `$ ` and trailing whitespace off every line,
/// leading and trailing blank lines dropped.
fn comparable(code: &str) -> String {
    let lines: Vec<&str> = code
        .lines()
        .map(|l| l.strip_prefix("$ ").unwrap_or(l).trim_end())
        .collect();
    lines.join("\n").trim_matches('\n').to_owned()
}

/// 220: check the repair answer against `fence`. `OK` or nothing → [`Repair::Declined`]. One
/// closed fence and nothing around it, a cell header (`surface::cell::parse`) of only
/// [`ALLOWED_KEYS`] and no invisible characters, the fence's language, no `validate` error in
/// chat and no changing command without `label`/`danger`, `#| danger` when the fence changes
/// state ([`changes_state`]), the original code modulo leading `$ ` and trailing whitespace
/// (and no `$ ` prompt left in) → [`Repair::Cell`]; anything else → [`Repair::Rejected`].
pub fn accept(answer: &str, fence: &PlainFence) -> Repair {
    let answer = answer.trim();
    if answer.is_empty() || answer.trim_end_matches('.').eq_ignore_ascii_case("ok") {
        return Repair::Declined;
    }
    let found = fences(answer);
    let [f] = found.as_slice() else {
        return Repair::Rejected(format!("{} fences in the answer", found.len()));
    };
    if f.range != (0..answer.len()) {
        return Repair::Rejected("text around the fence".to_owned());
    }
    let Some(cell) = cell::parse(f.info, &f.body) else {
        return Repair::Rejected("no `#|` header".to_owned());
    };
    if runnable(f.info) != runnable(&fence.lang) {
        return Repair::Rejected(format!("language changed to `{}`", f.info));
    }
    if let Some(l) = cell
        .header
        .iter()
        .find(|l| !ALLOWED_KEYS.contains(&l.key.as_str()))
    {
        return Repair::Rejected(format!("option `{}` is not allowed", l.key));
    }
    if !cell.options.unknown.is_empty() {
        return Repair::Rejected(format!("unknown options {:?}", cell.options.unknown));
    }
    if cell::lint(&cell)
        .iter()
        .any(|c| c.kind == cell::ChipKind::Invisible)
    {
        return Repair::Rejected("invisible characters".to_owned());
    }
    let ctx = cell::ValidateCtx {
        chat: true,
        names: cell.options.name.clone().into_iter().collect(),
        ..cell::ValidateCtx::default()
    };
    if let Some(p) = cell::validate(&cell, &ctx)
        .into_iter()
        .find(|p| p.severity == cell::Severity::Error || p.code == "changing_without_label")
    {
        return Repair::Rejected(format!("{}: {}", p.code, p.message));
    }
    if changes_state(&fence.body) && !cell.options.danger {
        return Repair::Rejected("a changing command without `#| danger`".to_owned());
    }
    if comparable(&cell.code) != comparable(&fence.body) {
        return Repair::Rejected("the code changed".to_owned());
    }
    if cell.code.lines().any(|l| l.starts_with("$ ")) {
        return Repair::Rejected("a `$ ` prompt is left in the code".to_owned());
    }
    Repair::Cell(answer.to_owned())
}

/// `text` with `fence` replaced by `cell` (a fence's text, no trailing newline), each
/// non-blank line of it indented like the original fence.
pub fn splice(text: &str, fence: &PlainFence, cell: &str) -> String {
    let pad = " ".repeat(fence.indent);
    let mut out = String::with_capacity(text.len() + cell.len() * 2);
    out.push_str(&text[..fence.range.start]);
    for (i, line) in cell.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if !line.trim().is_empty() {
            out.push_str(&pad);
        }
        out.push_str(line);
    }
    out.push_str(&text[fence.range.end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOY: &str = "The config is in place. Run the reload yourself:\n\n```bash\nssh -t -o HostKeyAlgorithms=ssh-ed25519 Admin 'sudo systemctl reload alloy && systemctl is-active alloy'\n```\n\nWhen it's done, tell me and I'll check the metrics.\n";
    const ALLOY_CODE: &str = "ssh -t -o HostKeyAlgorithms=ssh-ed25519 Admin 'sudo systemctl reload alloy && systemctl is-active alloy'";

    fn fence(lang: &str, body: &str) -> String {
        format!("```{lang}\n{body}\n```")
    }

    fn nudge(ordinal: u32, reason: &'static str) -> Verdict {
        Verdict::Nudge { ordinal, reason }
    }

    #[test]
    fn plain_fences_number_only_runnable_headerless_closed_fences() {
        let text = format!(
            "a\n{}\n{}\n{}\n{}\n{}\n```bash\nunclosed\n",
            fence("bash", "ls"),
            fence("js", "x()"),
            fence("bash", "#| cwd: /tmp\nls"),
            fence("rust", "fn x() {}"),
            fence("py", "print(1)"),
        );
        let got = plain_fences(&text);
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].ordinal, got[0].lang.as_str(), got[0].body.as_str()),
            (0, "bash", "ls\n")
        );
        assert_eq!(&text[got[0].range.clone()], "```bash\nls\n```");
        assert_eq!((got[1].ordinal, got[1].lang.as_str()), (1, "py"));
        // A fence inside a fence is content.
        assert!(plain_fences(&format!("````md\n{}\n````\n", fence("bash", "ls"))).is_empty());
    }

    #[test]
    fn cell_nudge_fires_on_the_alloy_reload() {
        assert_eq!(detect(ALLOY), nudge(0, "command"));
    }

    #[test]
    fn cell_nudge_prose_signals_before_or_after() {
        let before = format!("Run this in your terminal:\n\n{}\n", fence("sh", "make deploy"));
        assert_eq!(detect(&before), nudge(0, "prose"));
        let after = format!(
            "{}\n\nPaste the output here and I’ll read it.\n",
            fence("zsh", "ls ~/.ssh")
        );
        assert!(matches!(detect(&after), Verdict::Nudge { .. }));
        let curly = format!("{}\nTell me when it’s done.\n", fence("bash", "make"));
        assert!(matches!(detect(&curly), Verdict::Nudge { .. }));
        // A bare run instruction right before the fence.
        for lead in ["Run this:", "Then run the following:", "Now run:", "Done. Run these commands:"] {
            assert_eq!(detect(&format!("{lead}\n{}\n", fence("bash", "make"))), nudge(0, "prose"), "{lead}");
        }
    }

    #[test]
    fn cell_nudge_commands_need_a_run_lead_in_or_prose() {
        // A tutorial's install step: a changing command, nobody asked to run it.
        let tutorial = format!(
            "Install jq with Homebrew:\n{}\nThen pipe JSON into it.\n",
            fence("bash", "brew install jq")
        );
        assert_eq!(detect(&tutorial), Verdict::Skip);
        let led = format!("Then run the install on your Mac.\n{}\n", fence("bash", "brew install jq"));
        assert_eq!(detect(&led), nudge(0, "command"));
        // Weak prose is no signal on its own.
        for weak in ["Let me know if it works.", "This works on your machine."] {
            let text = format!("Install it:\n{}\n{weak}\n", fence("bash", "sudo apt install jq"));
            assert_eq!(detect(&text), Verdict::Skip, "{weak}");
        }
    }

    #[test]
    fn cell_nudge_negatives_skip() {
        let cases = [
            // No signal at all.
            format!("This lists the folder:\n{}\n", fence("bash", "ls -la")),
            // Placeholders.
            format!("Run this yourself:\n{}\n", fence("bash", "ssh -t <host> uptime")),
            format!(
                "Run this yourself:\n{}\n",
                fence("bash", "export TOKEN=YOUR_TOKEN\nsudo ls")
            ),
            format!("Run this yourself:\n{}\n", fence("bash", "sudo ls\n...")),
            // An illustration.
            format!(
                "For example, a reload looks like:\n{}\nRun it yourself.\n",
                fence("bash", "sudo systemctl reload x")
            ),
            format!("Restart it, e.g. with:\n{}\n", fence("bash", "sudo systemctl restart x")),
            // Too long: 8 lines.
            format!(
                "Run this yourself:\n{}\n",
                fence("bash", "sudo a\nb\nc\nd\ne\nf\ng\nh")
            ),
            // Already a cell.
            format!("Run this yourself:\n{}\n", fence("bash", "#| cwd: /tmp\nsudo ls")),
            // JS never promotes.
            format!("Run this yourself:\n{}\n", fence("js", "console.log(1)")),
        ];
        for text in &cases {
            assert_eq!(detect(text), Verdict::Skip, "{text}");
        }
    }

    #[test]
    fn cell_nudge_suppressors_read_only_the_clause_before_the_fence() {
        let text = format!(
            "You could also use a timer, for example. Now run this yourself:\n{}\n",
            fence("bash", "sudo systemctl restart x")
        );
        assert_eq!(detect(&text), nudge(0, "command"));
    }

    #[test]
    fn cell_nudge_heredocs_and_redirections_are_no_placeholders() {
        assert!(!placeholder("cat <<EOF > x\nhi\nEOF\n"));
        assert!(!placeholder("sort < in.txt > out.txt\n"));
        assert!(placeholder("curl https://<host>/x\n"));
    }

    #[test]
    fn cell_nudge_windows_stop_at_neighbouring_fences() {
        let text = format!(
            "For example:\n{}\n\nNow run this yourself:\n{}\n",
            fence("bash", "sudo ls"),
            fence("bash", "sudo systemctl restart x")
        );
        assert_eq!(detect(&text), nudge(1, "command"));
    }

    /// Amendment: the body decides, the ordinal breaks ties between identical bodies.
    #[test]
    fn cell_nudge_pick_follows_the_body_not_a_drifted_ordinal() {
        let text = format!(
            "{}\n{}\n{}\n",
            fence("bash", "ls"),
            fence("bash", "make"),
            fence("bash", "ls")
        );
        // Ordinal 0 sent for `make` (a client that counted fences differently): fence 1.
        assert_eq!(pick(&text, 0, Some("make  \n\n")).unwrap().ordinal, 1);
        // Identical bodies: the ordinal picks among them; a non-matching ordinal → the first.
        assert_eq!(pick(&text, 2, Some("ls")).unwrap().ordinal, 2);
        assert_eq!(pick(&text, 1, Some("ls")).unwrap().ordinal, 0);
        // A body matching nothing is not found, whatever the ordinal.
        assert_eq!(pick(&text, 0, Some("pwd")), None);
        // No body (the auto path): the ordinal.
        assert_eq!(pick(&text, 1, None).unwrap().body, "make\n");
        assert_eq!(pick(&text, 3, None), None);
    }

    fn alloy_fence() -> PlainFence {
        plain_fences(ALLOY).remove(0)
    }

    fn alloy_cell(header: &str) -> String {
        fence("bash", &format!("{header}\n{ALLOY_CODE}"))
    }

    #[test]
    fn cell_nudge_accepts_a_valid_repair_and_splices_it_in_place() {
        let f = alloy_fence();
        let cell = alloy_cell("#| cwd: ~\n#| label: Reload alloy on Admin\n#| danger\n#| expect: active");
        assert_eq!(accept(&format!("\n{cell}\n"), &f), Repair::Cell(cell.clone()));
        let out = splice(ALLOY, &f, &cell);
        assert!(out.starts_with(
            "The config is in place. Run the reload yourself:\n\n```bash\n#| cwd: ~\n"
        ));
        assert!(out.ends_with("```\n\nWhen it's done, tell me and I'll check the metrics.\n"));
        assert!(plain_fences(&out).is_empty(), "the fence is a cell now");
        assert_eq!(surface::cell_link::cells(&out).len(), 1);
        // A `$ ` prompt in the original is dropped by the repair.
        let prompted = format!("Run this yourself:\n{}\n", fence("bash", "$ make"));
        let p = plain_fences(&prompted).remove(0);
        assert!(matches!(
            accept(&fence("bash", "#| cwd: /tmp\nmake"), &p),
            Repair::Cell(_)
        ));
    }

    #[test]
    fn cell_nudge_list_fences_are_dedented_and_respliced_indented() {
        let text = "Steps:\n\n1. Run this yourself:\n\n   ```bash\n   sudo systemctl restart x\n   ```\n2. Tell me.\n";
        assert_eq!(detect(text), nudge(0, "command"));
        let f = plain_fences(text).remove(0);
        assert_eq!((f.indent, f.body.as_str()), (3, "sudo systemctl restart x\n"));
        let cell = "```bash\n#| cwd: /tmp\n#| label: Restart x\n#| danger\nsudo systemctl restart x\n```";
        assert_eq!(accept(cell, &f), Repair::Cell(cell.to_owned()));
        let out = splice(text, &f, cell);
        assert_eq!(
            out,
            "Steps:\n\n1. Run this yourself:\n\n   ```bash\n   #| cwd: /tmp\n   #| label: Restart x\n   #| danger\n   sudo systemctl restart x\n   ```\n2. Tell me.\n"
        );
        assert!(plain_fences(&out).is_empty(), "a cell now");
    }

    #[test]
    fn cell_nudge_declines_or_rejects_a_bad_repair() {
        let f = alloy_fence();
        assert_eq!(accept("OK", &f), Repair::Declined);
        assert_eq!(accept(" ok.\n", &f), Repair::Declined);
        assert_eq!(accept("  \n", &f), Repair::Declined);
        let rejected = |answer: String| matches!(accept(&answer, &f), Repair::Rejected(_));
        // No header.
        assert!(rejected(fence("bash", ALLOY_CODE)));
        // Another language.
        assert!(rejected(fence("python", &format!("#| cwd: /tmp\n#| danger\n{ALLOY_CODE}"))));
        // Missing cwd (a chat cell needs it), relative cwd.
        assert!(rejected(alloy_cell("#| danger")));
        assert!(rejected(alloy_cell("#| cwd: work\n#| danger")));
        // A changing command without danger (with or without a label).
        assert!(rejected(alloy_cell("#| cwd: /tmp")));
        assert!(rejected(alloy_cell("#| cwd: /tmp\n#| label: Reload alloy")));
        // Options outside the allowlist, unknown lines, invisible characters.
        assert!(rejected(alloy_cell("#| cwd: /tmp\n#| danger\n#| env: BASH_ENV=/tmp/x")));
        assert!(rejected(alloy_cell("#| cwd: /tmp\n#| danger\n#| python: /tmp/py")));
        assert!(rejected(alloy_cell("#| cwd: /tmp\n#| danger\n#| review")));
        assert!(rejected(alloy_cell("#| cwd: /tmp\n#| danger\n#| not a key!")));
        assert!(rejected(alloy_cell("#| cwd: /tmp\n#| danger\n#| label: Reload\u{200b} alloy")));
        // Edited code, a prompt left in.
        assert!(rejected(fence(
            "bash",
            "#| cwd: /tmp\n#| danger\nssh Admin 'sudo systemctl reload alloy'"
        )));
        assert!(rejected(alloy_cell("#| cwd: /tmp\n#| danger\n$")));
        assert!(rejected(fence("bash", &format!("#| cwd: /tmp\n#| danger\n$ {ALLOY_CODE}"))));
        // Prose around the fence, or two fences.
        let one = alloy_cell("#| cwd: /tmp\n#| danger");
        assert!(rejected(format!("Here:\n{one}")));
        assert!(rejected(format!("{one}\n{one}")));
        assert!(!rejected(one));
    }

    #[test]
    fn cell_nudge_book_spends_once_per_fence_and_gives_back() {
        let mut book = NudgeBook::default();
        assert!(book.take("a", 2));
        assert!(!book.take("a", 2), "tried before");
        assert!(book.take("b", 2));
        assert!(!book.take("c", 2), "cap");
        book.give_back("b");
        assert_eq!(book.count(), 1);
        assert!(book.take("b", 2), "a stale attempt may be tried again");
        book.give_back("nope");
        assert_eq!(book.count(), 2);
    }
}
