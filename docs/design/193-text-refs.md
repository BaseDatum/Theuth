# 193 — TEXT-REFS: move, copy, indent and clip text by reference

Status: contract. Base main 3f97017d. Evidence: knowledge `procedures/theuth-runtime-token-free-copy-paste-2026-11-11`, `facts/agent-editing-tool-libraries-survey-2026-11-11`, `facts/theuth-rust-crates-for-range-edit-and-reindent-2026-11-11`.

## Sky's words
- **2026-11-11 20:45:** "is there any tool in this harness that allows a model to do a copy paste like a human would without having to generate the tokens that it copied? for example, it can cut or copy a section from a file based on line number it gets an id for that clipboard entry, and then can paste it with the entry, a file target, and the line no to insert at. obviously with some reads throughout to locate where it wants to paste."
- **21:08:** "particularly interested in one that can fix a blocks indentation in one call"
- **~21:20:** "No this will need to be a built in tool in theuth, so I'm talking about rust libraries."
- **2026-11-12 11:35:** "so what tools would you create?" **11:41:** "I don't understand the purpose of rewrite" (theuth's answer: a different job, drop it from this design).
- **11:49, go:** "create a new worktree and get it done, do all the tools except the rewrite one as you suggested."

## Problem
A model moves text today by retyping it. `edit` needs the whole `old` block quoted to delete or replace it, and inserting needs the whole block typed into `new` or `write_file`. A move costs the block twice and a re-indent once: slow, paid in output tokens, and open to transcription slips. No theuth tool avoids this (verified 2026-11-11; the python runtime can do it with pathlib + `write_file`, but only in its own session and not for ACP children).

## Decisions
- **D1. Refs, not text.** The model names text it has already seen; the engine moves the bytes. The moved text never appears in the model's output.
- **D2. One ref grammar** for every tool (§1).
- **D3. Reads are snapshots.** A text `read_file` header carries a snapshot id. The engine keeps what each read returned, per session. A line ref carries the id of the read it came from and is mapped through a line diff to the file as it is now, so the model's own earlier edits do not break its line numbers. A call is refused only when the referenced lines themselves changed, or when the snapshot is unknown (§2).
- **D4. Tools.** `read_file` (header id), `edit` (new `range` mode), and four new tools: `move_text`, `copy_text`, `indent`, `clip` (§4). `rewrite` is not built.
- **D5. Indent is a uniform relative shift** (Vim's `]p`, ast-grep's rewrite), not language-aware auto-indent: the whole block moves so its base lands at the target depth and lines keep their depth relative to each other (§3).
- **D6. Symbol refs through a hook.** `tools` must not depend on `codeindex` (fs.rs:94-112, the `set_outliner` precedent). A `Syntax` hook installed by codeindex resolves `path#symbol` against the current text, widened over attached doc comments, attributes and decorators (today `code_symbol` puts `splice` at edit.rs:171-204 while its doc comment is on line 170), and reports multi-line string interiors.
- **D7. All or nothing.** A call writes every file it touches or none (§5).
- **D8. Two results.** `ToolOutput.content` (lib.rs:276) carries a short receipt for the model; `structured` carries the per-file unified diffs and snapshot ids (§6).
- **D9. Chaining.** Every write (the new tools, `edit`, `write_file`) records the new content as a snapshot and reports its id, so the model can keep addressing a file without reading it again.
- **D10. ACP reach.** The `t mcp --session` shim (crates/theuth/src/mcp.rs) offers `read_file`, `edit`, `move_text`, `copy_text`, `indent` and `clip` to ACP children, run in the child's theuth session through the normal registry path and approval policy (§7).
- **D11. Not built:** `rewrite` (its own design if ever); per-line hashes in read output (1-2 extra tokens per line on every read); language-aware auto-indent; separate delete and cut tools (`edit` with `range` and `new=""` deletes, `move_text` cuts, `clip` covers cut-now-paste-later); `{{clip:…}}` placeholders inside other tools' arguments; UI work (the new tools use the generic tool card).

## 1. Refs
```
ref     := path "#" anchor | "clip:" name
anchor  := lines "@" snap | symbol | "start" | "end"
lines   := "L" n | "L" n "-L" m              1-based, inclusive, n <= m
snap    := 8 lowercase hex                   from a read_file header or a tool receipt
symbol  := ident ("." ident)*                splice, Reader.load
name    := [A-Za-z0-9_.-]{1,64}
```
- `path` is split from the anchor at the LAST `#`. Paths resolve exactly as `read_file`/`edit` resolve them (session cwd, absolute allowed, no `~`), through the same helper, so no new tool reaches a path `edit` cannot.
- `edit.range`, `indent.range` and `indent.match` take an anchor without `path#` (the path is the tool's `path` argument).
- Targets (`before`, `after`) take a single-line ref or a symbol ref; `at` takes `path#start` or `path#end`. Exactly one of the three. `before=sym` lands above the symbol's widened start; `after=sym` below its end.
- `clip:name` is valid only as `copy_text.src`.

## 2. Snapshots
- **Id:** the first 8 hex chars of sha256 of the file's bytes (sha2 is already a tools dependency).
- **Header:** every text read gets `, @<id>` after the byte count: `[read_file <path>: N lines, B bytes, @1a2b3c4d; showing lines a–b]`. Not for images, rendered documents or `outline=true`.
- **Recorded:** the full file text for each text read (whatever window was shown) and for each write by `edit`, `write_file`, `move_text`, `copy_text`, `indent`.
- **Store:** in memory, keyed by (session, canonical path, id); the last 8 per file; 32 MiB per session; 256 MiB per engine, LRU; files over 4 MiB are not recorded (their header has no id and a line ref into them is refused with that reason). Lost on engine restart.
- **Resolving `Ln-Lm@id` against the current text C:**
  1. No snapshot S for (session, path, id): refuse, "unknown snapshot @id for <path> (engine restarted or snapshot evicted): read the file again".
  2. sha(C) == id: use n..m as given, bounds-checked.
  3. Otherwise diff S to C by lines with `similar` (workspace dep). If every line n..m of S sits in an Equal op and they map to a contiguous n'..m' in C, use n'..m'. Otherwise refuse, "lines n-m of <path> changed since @id", with C's current id and the current text of the mapped region (at most 40 numbered lines), so the model can retry without another read.
- Single-line target refs map the same way. Symbol refs and `start`/`end` resolve against C and need no id.

## 3. Indent rules (shared by `indent`, and by `move_text`/`copy_text` with `indent`)
- **Unit:** `.editorconfig` via `ec4rs` 1.2 (new dependency, Apache-2.0: `indent_style`, `indent_size`, `tab_width`) for the target file; otherwise detected from the file: tabs when more indented non-blank lines start with a tab than with a space; otherwise the most common positive indentation step between consecutive non-blank lines among 2, 3, 4, 8 (ties and nothing found: 4). Tab width for measuring: `tab_width`, else a numeric `indent_size`, else 4.
- **Block base:** the smallest leading-whitespace width among the block's non-blank lines that are not string interiors.
- **Shift:** every non-blank line that is not a string interior gets width - base + target. Leading whitespace is re-rendered in the target file's style (tabs: width / tab_width tabs then the remainder as spaces; spaces: width spaces).
- **Blank and whitespace-only lines** become empty.
- **Multi-line string interiors** (from the `Syntax` hook: lines whose leading whitespace lies inside a multi-line string literal) are left byte-for-byte, because re-indenting them changes the string's value (ast-grep issue 3077, closed wontfix, does exactly that). The receipt names how many were kept. Languages without a parser get no protection and the receipt says so.
- **Negative shifts** that would cut into a line's text are refused, naming the first such line.
- **`match` base for paste targets** (`indent="match"`, the default for `move_text`/`copy_text`), measured in the target after any same-file removal:
  1. target is a symbol: the indentation of the symbol's first (widened) line, so the block lands as its sibling;
  2. target is `start`/`end`: column 0;
  3. target is a line: with prev = the last non-blank line above the insertion point and next = the first non-blank line at or below it:
     - prev ends with `{`, `(`, `[` or `:` (ignoring a trailing comment): prev's indentation plus one unit;
     - else next exists and is at least as deep as prev (or there is no prev): next's indentation;
     - else prev exists: prev's indentation;
     - else column 0.
  This rule is deterministic, not language-aware. The receipt always shows the line on each side of the inserted block and the shift applied, so a wrong guess costs one `indent` call. `indent="keep"` pastes as is; `"+N"`/`"-N"` shifts by N units from the source's own depth.

## 4. Tools
### `read_file` (changed, ReadOnly)
- Header id per §2; records the snapshot. Description (fs.rs:126) and the runtime prompt line that describes the header (crates/agent/src/repl.rs, the `numbers=False` sentence) say the id exists and that line refs take it.
- Every detector of the header text (the `edit`/`write_file` refusal of content carrying a read_file header, header-matching tests anywhere in the workspace) accepts the new form.

### `edit` (changed, Mutating)
- New optional `range` (an anchor: `Ln@id`, `Ln-Lm@id` or a symbol name), mutually exclusive with `old`. `new` replaces the whole lines, line breaks included; `new=""` deletes them. A non-empty `new` without a final newline gets the file's newline, except at the end of a file that had none.
- `edits` entries take `range` or `old`; one list uses one kind (mixing is refused). Range entries resolve against their snapshots, must not overlap, and apply together bottom-up.
- Range mode answers with the §6 receipt. `old` mode keeps today's unified-diff answer and appends one line `<path> @<id>`.
- `write_file` likewise appends `<path> @<id>` and records the snapshot.

### `move_text` (new, Mutating)
- Args: `src` (lines or symbol ref), exactly one of `before`/`after`/`at`, `indent` (`"match"` default, `"keep"`, `"+N"`, `"-N"`).
- Removes the source lines and inserts them at the target in one all-or-nothing write; same file or two files. Refuses when the target lies inside the source. `at=path#start|end` on a missing file creates it (parent dirs too).
- Inserted lines take the target file's line ending (LF or CRLF by majority). A block whose last line had no newline gets one when it lands before another line; a target whose last line has no newline gets one before an appended block. A BOM is kept.

### `copy_text` (new, Mutating)
- As `move_text`, the source untouched. `src` may also be `clip:name`.

### `indent` (new, Mutating)
- Args: `path`, `range` (lines or symbol anchor), `by` (signed units), `match` (a line anchor in the same file: its indentation becomes the base, then `by` units are added). At least one of `by`/`match`.
- §3 rules. Receipt example: `indented a.py:40-52 by +4 spaces (1 level) · a.py @9c0d12aa · 2 lines inside a multi-line string kept`.

### `clip` (new, ReadOnly)
- Args: `name`, `src` (lines or symbol ref). Stores the resolved text as it is now under `name` in the session's clip store, replacing any clip of that name.
- Caps: 1 MiB per clip, 64 clips, 16 MiB per session; in memory; lost on restart. `copy_text(src="clip:x")` with an unknown name is refused, listing the clips that exist.

## 5. Writes
- Every new file content is computed in memory first. Before each write the file is read again and must still be what the call resolved against, or the call is refused. Writes use `edit::write_atomic` (edit.rs:697). If a later write fails, the files already written get their previous contents back, and the error says so.
- Each file keeps its line-ending style, its final-newline state (except where §4 adds one) and its BOM. Non-UTF-8 files are refused.

## 6. Results
- **content:** one receipt line, then ` · <path> @<id>` for every file written, then for paste targets the line above and the line below the inserted block, numbered. No diff. Example:
  `moved 111 lines edit.rs:150-260 -> text.rs:88-198 (indent +4) · edit.rs @9c0d1234 · text.rs @77ab03ef`
  `  text.rs:87   }`
  `  text.rs:199  impl Reader {`
- **structured** (the python runtime's value; set `RETURNS` accordingly): `{op, lines, bytes, notes: [str], files: [{path, snapshot, diff}]}`, `diff` unified with 3 lines of context.
- Errors set `is_error` and say what to do next (fresh context per §2 where it applies).

## 7. ACP reach
- An RPC method for the shim runs one tool in the child's theuth session: allowlist `read_file`, `edit`, `move_text`, `copy_text`, `indent`, `clip`; the session's cwd; the normal registry dispatch, so the approval policy and secret handling apply unchanged. The shim lists the six tools with the registry's own schemas and descriptions, beside `list_skills`, `load_skill`, `send_message`.
- Bound: if the existing shim/RPC pattern cannot run a registry tool in another session's context without new supervisor plumbing, stop at a written note in the as-built naming what is missing. Do not build that plumbing in this design.

## 8. Tests (crates/tools unless noted)
- Ref parsing: every form, the last-`#` split, malformed refs.
- Snapshots: header id; recording on read and on every write; mapping across insertions above, inside-file edits below, and edits elsewhere; refusal when referenced lines changed (with fresh context); unknown id; the 4 MiB limit; LRU caps.
- `edit` range: replace, delete, multi-entry bottom-up, overlap refused, mixing refused; `old` mode unchanged plus the id line; `write_file` id line.
- `move_text`/`copy_text`: across files, within a file both directions, target inside source refused, new file via `at`, CRLF target, missing final newline, BOM, rollback when the second write fails (unwritable target), receipt seams.
- `indent`: spaces, tabs, `.editorconfig`, detection, `match`, negative refusal, blank lines emptied, `match` paste rules 1-3 (each branch), Python and Rust string interiors kept (codeindex test through the hook).
- Symbol refs (codeindex): widened over `///`, `#[...]` and Python decorators; `Type.method`; ambiguity lists candidates; unparsed language refused.
- `clip`: store, replace, caps, unknown name lists names, paste via `copy_text`.
- ACP shim: the six tools are listed and one call round-trips (or the as-built note per §7).
