# 193 — TEXT-REFS: as built

Contract: [193-text-refs.md](193-text-refs.md). Rebased in R2 onto main 9c439c3c (the wire split). All of §1-§8 is built, §7 included. §7's one supervisor change is the R2 seat fix (B1), which the orchestrator allowed despite §7's "no new supervisor plumbing".

The file:line references in the sections below are from R1. For what R2 changed, see [`## R2`](#r2); for R3, [`## R3`](#r3) at the end. R2 and R3 cite symbols, not lines.

## What was built (file:line at the docs commit)

### `crates/tools/src/textref.rs` (new): refs, snapshots, clips, the hook, the write
- §1 ref grammar: `Anchor` :20, `Ref` :40, `parse_ref` :55 (split at the last `#`), `parse_anchor` :75, line anchors `parse_lines` :98, clip names `check_clip_name` :143.
- §2 ids: `snapshot_id` :163 (first 8 hex chars of sha256 of the bytes), `MAX_SNAPSHOT_BYTES` :160 (4 MiB).
- `Doc` :170: a UTF-8 file as LF lines with its BOM, CRLF-majority rule (the one `edit` uses) and final-newline state. `render()` writes them back. `normalize` :267 and `split_lines`/`join` :246/:256.
- §2 store: `Snapshots` :319, keyed by (session, canonical path, id), with `Caps` :291 (8 per file, 32 MiB per session, 256 MiB per engine, LRU at each level). Process-wide `SNAPSHOTS` :399. `record_doc` :403, `record_bytes` :416, `stamp` :429 (D9 id line for `edit`/`write_file`).
- §2 resolution: `map_lines` :453 runs steps 1-3. A refusal names C's id, records C under it so the id can be used at once, and shows the mapped region of C, numbered, capped at 40 lines (`numbered` :557).
- §4 clip store: `put_clip` :609 and `get_clip` :636. Caps `CLIP_MAX`/`CLIP_COUNT`/`CLIP_SESSION` :582-586.
- D6 hook: `SymbolSpan` :655, `SyntaxInfo` :669, `trait Syntax` :680, `set_syntax`/`syntax` :689/:694 (the `set_outliner` precedent), `analyze` :700 (runs in `spawn_blocking`), `find_symbol` :712.
- §5: `Planned` :751 and `commit` :761. Every file is re-read and compared with what the call resolved against, then written with `edit::write_atomic` in order. If a write fails, the earlier files are put back (or removed, if the call created them), and the error says so.

### `crates/tools/src/text.rs` (new): the tools and §3
- §3: `style` :27 (ec4rs `.editorconfig`, else `detect_tabs` :55 / `detect_step` :69), `width` :93, `render` :111, `base` :122, `shift` :141 (string interiors byte-for-byte, blank lines emptied, negative cut refused), `opens` :179 and `line_depth` :194 (rule 3), `describe` :225, `strings_note` :243.
- `move_text` `MoveText` :718 and `copy_text` `CopyText` :738 share `paste` :526. Targets come from `Target::parse` :466, the seam lines from `seams` :699.
- `indent`: `Indent` :777, args `IndentArgs` :761 (`match` is serialised as `match`).
- `edit` range mode: `edit_ranges` :848, called from `edit.rs:681`.
- `clip`: `ClipTool` :935.
- Shared plumbing: `resolve` :279, `block_of` :387, `write_docs` :313 (commit, then record the snapshots, then the 3-line-context diffs), `structured` :357 (§6 `{op, lines, bytes, notes, files:[{path, snapshot, diff}]}`).

### Changed in `crates/tools`
- `fs.rs`:
  - `read_file` records the snapshot (:146) and writes `, @<id>` after the byte count (`page` :250, :307).
  - `paginate` keeps its signature for `artifact_read`, which gets no id.
  - The description at :127 says what the id is and which tools take it. The first sentence is unchanged, so the runtime index summary (`repl.rs` `index_lines_and_guidance_name_images`) still holds.
  - `write_file` stamps both answers (:537, :573).
- `edit.rs`:
  - `EditOp.old` is now `Option`, and `EditOp.range` and `EditArgs.range` are new (:508, :531). Validation is in `Ops` :560 / `ops()` :567; dispatch is at :681.
  - `old` mode keeps its diff answer and adds the id line (:745, :760).
- `seat.rs`: `real` is `pub(crate)`. It gives the canonical path that snapshots are keyed by.
- `registry.rs`: :2221-2224 registers the four tools; the builtins array goes from 16 to 20.
- `Cargo.toml`: ec4rs (see deviations).
- `lib.rs`: `pub mod text; pub mod textref;`.

### `crates/codeindex/src/syntax_hook.rs` (new), installed at `lib.rs:312` next to the outline hook
- `analyze` :25:
  - The language comes from the extension. The definitions come from `parse::parse`: tags, parents, and Rust impl methods attached to their struct.
  - The qualified name is the parent chain joined by `.`.
  - A tree-sitter tree is parsed to widen each symbol (`widened_start` :76, `attached` :105): Python `decorated_definition`; TS/JS `export_statement`, `/** */` and `decorator`; Rust `attribute_item`, `///` and `/** */`; Go `//` directly above. Nothing is attached across a blank line.
- `string_lines` :130: the rows after the opening row of every multi-line string literal. Kinds: Rust `string_literal`/`raw_string_literal`; Python `string`; JS/TS `string`/`template_string`; Go raw/interpreted strings.
- `code_symbol` is untouched; the widening exists only in this hook.

### `crates/agent`
- `repl.rs:104-108`: the runtime prompt sentence now shows the header with `@<id>` and says which tools take line refs.
- `supervisor.rs:2865`: doc of `mcp_shim` lists the six tools.
- `sensor/bridge.rs` passes `read_file` output through unparsed; `dreaming/sensor.py` `strip_header` drops the whole first line. Neither needed a change.

### §7 ACP reach (`crates/rpc`, `crates/theuth`)
- `rpc/src/api.rs:973` adds `session_call_tool(session, name, args) -> ToolOutput` (`with_extensions`).
  - The handler is at :5839. It refuses any name outside `SESSION_TOOLS` (:4304: `read_file`, `edit`, `move_text`, `copy_text`, `indent`, `clip`) with INVALID_PARAMS, then `authorize(ext, session)` (the `send_message` pattern), then the registry's normal `Invoke(ToolCall{session, ..})`.
  - So the session's seat (cwd, write root), the approval gate and secret expansion and redaction apply unchanged. `ApprovalRequested` is raised for the child session and answered by any client, as for any session.
- `theuth/src/mcp.rs`:
  - `list_tools` takes the existing global `list_tools` RPC, keeps the six, and builds each MCP tool from the registry's own name, description and schema, after `list_skills`/`load_skill`/`send_message`.
  - `call_tool` forwards those six to `session_call_tool` (:131). An `is_error` output becomes an MCP tool error.
  - The module doc and `get_info` text are updated, and so are the `t mcp` docs in `theuth/src/main.rs:178` and :1324.
- `crates/desktop/ui/src/rpc/api.ts` adds `session_call_tool` to `Api` and to `METHODS` (sorted). Otherwise `api.test.ts` would fail once `bindings/methods.json` is regenerated.

## Decisions where the contract is silent
1. **Snapshot text** is the LF-normalised body without BOM (the form `Doc` works on). The id is still the sha of the raw bytes. A non-UTF-8 text file gets no id, since refs refuse non-UTF-8 anyway.
2. **Order of §2 steps**: step 1 (unknown id → refuse) comes before step 2, as written, even when the file's current sha equals the id.
3. **Line mapping** diffs the line vectors without terminators (`similar::capture_diff_slices_deadline`, Myers, 2 s deadline). A missing final newline therefore never counts as a change to the last line. If the deadline is hit, the result is a refusal, never a guess.
4. **Refused region**: the lines of C that the snapshot's lines map to. An Equal line maps to its new line; a Delete/Replace line maps to its op's new range. At most 40 numbered lines are shown, with C's id already recorded.
5. **`L<digit>…` is always a line anchor**. So `L5` without `@id` is refused ("needs the snapshot") rather than read as a symbol named `L5`. A symbol is `ident(.ident)*` with ident `[A-Za-z_$][A-Za-z0-9_$]*`.
6. **Symbol matching**:
   - an exact qualified match wins;
   - otherwise there must be exactly one symbol whose qualified name ends in `.<name>`;
   - several matches are refused, listing `name (kind, lines a-b)` for each.
   - Rust methods are qualified by their struct only when the struct is in the same file (codeindex's `attach_impl_methods`).
7. **`start`/`end`** are valid only in `at`. As a `src`, `edit.range` or `indent.range` they are refused.
8. **Final newline on paste**:
   - A block appended at the end of a non-empty file leaves the file with a final newline if it had one or the block's last line had one. So no newline is ever removed.
   - A block appended to an empty or new file keeps its own ending.
   - A move that takes a file's last lines keeps the source's final-newline state.
9. **`edit` range and the end of file**: when the bottom range includes the last line and `new` is non-empty, the final newline becomes `had || new ends with "\n"` (the contract's rule, applied once).
10. **Moves onto their own boundary**: `before` the first source line or `after` the last is allowed, and is a no-op apart from re-indent. Only a target strictly inside the source is refused. Files whose bytes do not change are not written, but their ids are still reported.
11. **Rule 1 for `after=Symbol`** measures the symbol's widened first line, located again after a same-file removal.
12. **Tab width without `tab_width`** is a numeric `indent_size` (§3). A tab in a file whose `.editorconfig` says `indent_size = 3` therefore measures 3 columns.
13. **Unit**:
    - `indent_style` from `.editorconfig` when set, else detected;
    - `indent_size` when numeric, or `tab` → tab width;
    - otherwise the tab width for tab style, or the detected step for spaces.
    - `ec4rs::properties_of` is called on the absolute target path, and its fallbacks are not applied.
14. **Trailing-comment stripping** for rule 3 drops `//…` and a `#…` preceded by whitespace or at column 0. It does not see string literals (marked `ponytail:` at `text.rs:179`); the seam lines in the receipt show any miss.
15. **Receipts**:
    - A paste says `(indent +N)` (columns, signed) when the block moved, `(indent kept)` for `"keep"`, and nothing when the shift was 0.
    - `indent` says `by +4 spaces (1 level)` or `by -1 tab (1 level)`, with levels only when the shift is a whole number of units.
    - Without a parser, the note is `no parser for this language: multi-line strings not protected`.
    - Paths in receipts are as the model wrote them. `edit`'s range mode uses the resolved path, as its `old` mode already did.
16. **`edit` range receipts**:
    - `replaced <path>:a-b (k lines) with m lines · <path> @id`;
    - `deleted <path>:a-b (k lines) · …`;
    - `edited <path>: N ranges, -r +a lines · …`.
    - `structured.op` is `edit`. In `old` mode, `structured` gains `snapshot`, and so does `write_file`.
17. **Files over 4 MiB** get neither an id line nor a snapshot on write; receipts say `(over 4 MiB, no snapshot)`. `read_file` still records the snapshot for `outline=true` (harmless; the header shows no id).
18. **Clips**:
    - A clip keeps the string-interior flags of its source lines, so a later `copy_text(clip:…)` still protects them.
    - `move_text` refuses `clip:` (only `copy_text` pastes clips), and so does `clip` as a source for another clip.
    - The clip name is checked before the source is read.
19. **`paste` path checks**:
    - `move_text`'s source and every target go through `seat::writable`. The sources of `copy_text` and `clip` go through `seat::resolve`, like `read_file`.
    - All of them go through `refuse_cell_record`.
20. **Write order** for a two-file move: source first, then target. A failed target write therefore restores the source.
21. **§7 details**:
    - specs come from the existing global `list_tools` RPC (no second listing method);
    - the allowlist const lives in `rpc::api`;
    - call id is `mcp-<uuid v7>` and `quiet` is `false`;
    - the shim forwards `content` and `is_error` only (§6 says the model reads `content`);
    - a failed `list_tools` at list time leaves only the shim's own three tools (R2, integ #6).

## Deviations
- **ec4rs comes from git, not crates.io.** The root `[workspace.dependencies]` has `ec4rs = { git = "https://github.com/TheDaemoness/ec4rs", rev = "14bbca047324cedd5791b89f858eb5e17e6b0b3c" }`, and `crates/tools` uses `{ workspace = true }` (R2).
  - On this machine `static.crates.io` answers 403 (Zscaler) for every crate download, while github.com is reachable.
  - The rev is the 1.2.0 release commit (same code, Apache-2.0, no default dependencies).
  - Switch the line to `ec4rs = "1.2"` once the registry is reachable. `Cargo.lock` then changes the `source` line and gains a `checksum` line.
- **Commits**: the tools halves of deliverables 1 and 2 are one commit. `edit`'s range mode reuses `text.rs`'s resolve/write/receipt helpers, so the two do not compile apart without duplicated code.

## Known limits (§7)
- `list_tools` is global, so a tool hidden or capped for the child is still listed; the call is then refused with the registry's own text.
- The shim's client has its own 330 s request timeout (R2, B2): the 300 s default approval wait plus 30 s. A harness-side MCP timeout can still cut a call earlier, while the approval stays pending.
- Image outputs (`files`) are not forwarded through the shim.

## Tests (new)
- `tools` `textref::tests`:
  - `refs_parse_every_form_and_split_at_the_last_hash`
  - `malformed_refs_are_refused_with_the_forms`
  - `snapshots_keep_the_last_8_per_file_and_honour_byte_caps_lru`
  - `symbols_resolve_exact_then_by_suffix_and_list_ambiguity`
  - `docs_round_trip_crlf_bom_and_missing_final_newline`
- `tools` `text::tests` (a fake `Syntax` hook on `.fk` files):
  - `read_header_carries_the_snapshot_id_but_not_over_4_mib`
  - `line_refs_follow_own_edits_and_refuse_changed_lines_with_context`
  - `edit_range_replaces_deletes_and_refuses_overlap_and_mixing`
  - `edit_old_mode_and_write_file_end_with_a_usable_id_line`
  - `move_across_files_and_within_one_both_ways`
  - `paste_creates_files_and_keeps_crlf_bom_and_newline_state`
  - `move_puts_the_source_back_when_the_target_write_fails`
  - `indent_uses_detected_tab_and_editorconfig_units`
  - `indent_match_and_negative_refusal`
  - `paste_match_follows_each_rule`
  - `symbol_refs_widen_and_string_interiors_are_kept`
  - `clips_store_replace_paste_and_name_what_exists`
- `codeindex` `syntax_hook::tests` (the real hook):
  - `rust_doc_and_multiline_attribute_widen`
  - `rust_impl_method_is_qualified`
  - `python_decorators_widen`
  - `ambiguous_method_lists_both`
  - `no_parser_is_none`
  - `rust_string_lines`
  - `python_string_lines`
  - `indent_keeps_rust_string`, `indent_keeps_python_string`, `indent_keeps_typescript_template`, `indent_keeps_go_raw_string` (end to end through `tools::text::Indent`)
- `theuth` `mcp::tests::tools_proxy_to_the_engine` is extended:
  - nine tools are listed;
  - `indent` carries the registry's description and schema;
  - an `indent` call round-trips with the child session id and args;
  - a `clip` `is_error` output comes back as a tool error;
  - `shell` is neither listed nor forwarded.
- Existing tests changed only where the new id breaks an exact match:
  - `fs::tests::header` (the helper now appends `, @<id>`);
  - `fs::tests::write_creates_with_parents` and `write_overwrite_reports_diff_and_identical_is_a_no_op` (id line);
  - `edit::tests::diff_is_capped_at_200_lines` (the id line follows the truncated diff);
  - test initialisers that gained `range: None`.

## Gates
Logs are in `/tmp/193/`. Every Rust command ran with `BUILD_LABEL=193 ~/.theuth/bin/build-lock mbx …`, except the bare test runs.
- **fmt**: `cargo fmt -p tools -p codeindex -p agent -p rpc -p theuth`. Afterwards `git status` shows only this design's files.
- **check**: `mbx check -p tools -p codeindex -p agent -p rpc -p theuth --tests` gives 0 errors. There is 1 warning, already on main: `unused import sqlx::Row` at `crates/codeindex/src/map/export.rs:627`, a file this design does not touch. This run used the final `Cargo.lock`, which adds the 6 lines for ec4rs and nothing else.
- **clippy, tools and codeindex** (`--no-deps --tests -- -D warnings`):
  - `tools`: clean, after two test-code fixes.
  - `codeindex`: fails only on the same `map/export.rs:627` unused import, which is already on main. There is no finding in `syntax_hook.rs`.
- **clippy, agent, rpc and theuth** (`--no-deps --tests -- -D warnings -A clippy::chunks_exact_to_as_chunks -A deprecated`): exit 0, 0 warnings.
- **tests, tools and codeindex**: `mbx test -p tools -p codeindex --no-run`, then bare `mbx test -p tools -p codeindex --no-fail-fast`.
  - `tools` lib: 258 passed, 0 failed, 3 ignored.
    - In the final run 256 passed and 2 failed: `shell::tests::concurrent_callers_share_one_probe` and `shell::tests::probe_child_has_no_controlling_terminal`.
    - Both pass when run on their own (2/2). They are flakes of the login-shell probe under load; `shell.rs` is not touched.
  - `codeindex`:
    - lib: 95 passed, 1 ignored.
    - `codemap`: 7 passed.
    - `locate`: 1 passed.
    - `index`: 21 passed, 2 failed. `map_and_symbol` and `impact_on_this_repository` are known reds on main. `project_profile_on_this_repository` (a ≤ 20 ms timing assert) failed under load in 2 of 3 runs and passes alone.
  - The new tests ran a second time: `-- text:: textref:: syntax_hook::` gives 17 passed in tools and 11 in codeindex.
- **tests, agent, rpc and theuth**: `mbx test -p agent -p rpc -p theuth --no-run`, then `mbx test -p agent -p rpc -p theuth -- repl:: mcp::tests::tools_proxy_to_the_engine`.
  - `agent` `repl::`: 22 passed, 1 ignored. This includes `index_lines_and_guidance_name_images` and the prompt/footer size tests.
  - `theuth` `tools_proxy_to_the_engine`: 1 passed.
- **TypeScript**: not run. See Deviations.
- **Smoke**: the `text::tests` drive the real tools end to end on real files. The four `syntax_hook::indent_keeps_*` tests run the real tree-sitter hook through `indent`, on Rust, Python, TypeScript and Go files.

## R2

R2 rebased onto main 9c439c3c and fixed the review findings (`review-193-integ.md`, `review-193-core.md`) as decided in the R2 brief. The history is now: the contract, then `feat(193)` (tools, codeindex, prompt, name-keyed lists), then `feat(193)` (ACP reach: rpc, theuth, the supervisor seat fix), then this note.

### A. Rebase
- `session_call_tool` moved into `crates/rpc/src/api/tooling.rs` (`ToolingApi`): the declaration, the body, and `pub const SESSION_TOOLS` beside them, re-exported as `rpc::api::SESSION_TOOLS`.
- The shim calls `list_tools` and `session_call_tool` through `rpc::api::clients::ToolingApiClient`, so the session id is now parsed as a `SessionId`.
- Nothing generated was edited by hand.

### B. Majors
1. **integ #1, ACP child seat: fixed** in `supervisor.rs` `acp_session`, right after `inherit_approvals`. This is the 1-3 line correctness fix the orchestrator allowed despite §7's "no new supervisor plumbing".
   - If the child has no write root, it gets the parent's (`seat::set_write_root`); then `seat::set_cwd(id, Some(cwd_path))`.
   - The first attempt used `seat::inherit`, which is a no-op once `inherit_approvals` has seated the child from its row. The new test caught that.
   - Test: `supervisor::tests::acp_child_seat_is_its_spawn_cwd_with_the_parents_write_root`. Relative shim paths therefore resolve in the child's own worktree, because the registry resolves through this seat.
2. **integ #2, shim timeout: fixed.**
   - `link_cli::connect_with_timeout` is a new sibling; `connect` behaves as before for every other caller.
   - The `--session` shim (first dial and redials) uses `SESSION_REQUEST_TIMEOUT` = 330 s, a constant: the 300 s default `tools.approval_timeout_secs` plus 30 s. The shim cannot read the config cheaply.
   - A harness-side MCP timeout can still cut a call earlier.
3. **core #1, snapshot lock: fixed.**
   - `textref::record` and `lookup` build the canonical key and the `Arc<str>` before taking `SNAPSHOTS`. `Snapshots::record` takes `(PathBuf, String, Arc<str>)`.
   - `CLIPS` stores `Arc<Clip>`; `put_clip` builds and measures the clip first, and `get_clip` clones only the `Arc` under the lock.

### C. Minors and nits: 28 fixed (16 findings, 8 nits, 4 name-keyed sites), 3 declined
- **core #2: fixed.**
  - `|by|` and `"+N"`/`"-N"` above 64 levels are refused (`check_levels`), as is any resulting width above 1024 columns (`MAX_WIDTH`, in `shift`).
  - `.editorconfig` sizes are capped at 1024, so the arithmetic cannot overflow.
- **core #3: fixed.** `textref::cut_line` cuts every line in a refusal (`numbered`), a seam (`seams`) and a shift refusal at 200 chars with ` …[+K chars]`.
- **core #4: fixed.**
  - `move_text` and `indent` say string protection applies "where codeindex has a parser (rust, typescript, javascript, python, go)".
  - `move_text` now says the line ref follows the file's later changes through the snapshot id.
  - `write_file` names its `<path> @<id>` line.
- **core #5: fixed.** `text::resolve` refuses a hook span outside `1 <= start <= end <= lines` instead of slicing.
- **core #6: fixed.** `map_lines` is async: the Myers diff runs in `spawn_blocking` and is cached per snapshot id in `textref::Diffs`, held in the call's `Parsed`. All `edits` entries into one snapshot therefore share one diff.
- **core #7: fixed.**
  - Clips are capped at 128 MiB per engine, LRU across sessions (`CLIP_ENGINE`, `Clips::put`), and dropped with their session by `textref::forget_clips` from the registry's `Forget` handler, beside `seat::forget`.
  - Snapshot rows cost `text + path + 64` bytes (`Snap::cost`), and the `ponytail:` comment now gives the real row bound.
- **core #8 / integ #8, ec4rs: fixed.** It is a root workspace dependency pinned by `rev = 14bbca04…`; see Deviations for the `checksum` line.
- **integ #3: fixed.** Test `rpc::api::tests::session_call_tool_allowlist_precedes_authorize`:
  - `shell` gets -32602 before authorize, even for an unknown session;
  - none of the six gets the allowlist refusal.
- **integ #4: fixed.** A comment, attribute or decorator attaches only when it starts its line (`starts_line` in `syntax_hook.rs`).
- **integ #5: fixed.** Before the sibling walk, the hook climbs from the tag node to a single-declaration `type_declaration` (Go) or `lexical_declaration`/`variable_declaration` (TS/JS), then to `export_statement` or `decorated_definition`.
- **integ #6: fixed.** A failed `list_tools` lists the shim's own three tools and logs to stderr. Test: `mcp::tests::failed_list_tools_lists_only_the_own_three`.
- **integ #7: fixed.** `syntax_hook::register()` runs before codeindex's `enabled` check.
- **integ #9: fixed.** Stale texts corrected:
  - the `mcp.rs` module doc (nine tools) and its test doc;
  - `crates/acp/src/agents.rs` (the `mcp` field);
  - `site/content/docs/automation/delegation.mdx`;
  - the `session_call_tool` doc: allowlist and authorize refusals are RPC `Err`; the tool's own failure is `is_error`.
- **integ #10: fixed.** When every candidate has the same qualified name, `find_symbol` says "use a line ref" and does not advise qualifying.
- **integ #11 + core NIT: fixed, partly.**
  - A panic in the hook is its own refusal (`the syntax parser failed on …; use a line ref`), not "needs a parser"; `textref::analyze` returns `Result`.
  - `parse::parse_tree` reuses the import-query tree for the hook, which removes the third parse. Two parses remain, because tree-sitter-tags runs its own parser and exposes no tree.
- **integ #12: fixed.** `SyntaxInfo.has_errors` comes from `root.has_error()`. A symbol ref into such a file adds the receipt note `file has syntax errors: symbol span may be approximate`.
- **core NITs: all fixed.**
  - `edit_file` uses `textref::normalize` (one CRLF rule).
  - `paste` matches on `src_doc.take()`; there is no `expect`.
  - A shift refusal names the source file and its real line number (`Block.origin`).
  - `write_docs` renders each doc once.
  - Line anchors must match `^L[0-9]+(-L[0-9]+)?(@[0-9a-f]{8})?$` (`is_line_anchor`), so `L2Cache` is a symbol.
  - The rollback tests return early when euid is 0.
  - `structured.bytes` counts the bytes written, CRLF included (`rendered_bytes`).
  - `text.rs`'s tests moved verbatim to `crates/tools/src/text/tests.rs`.
- **Name-keyed lists: fixed.**
  - `double/src/class.rs`: path-shaped approval class; `touched_paths` gives `move_text` its source and target paths joined by `+`, `copy_text` its target, and `indent` its `path`.
  - `acp/src/render.rs`: `move_text`/`copy_text`/`indent` → `ToolKind::Edit`, `clip` → `Read`.
  - `cells/src/actor/chat.rs`: `edited_paths` gives the same path sets.
  - `agent/src/loop_.rs`: `written_paths` covers `write_file`, `indent`, `move_text` and `copy_text`. `edit` was never tracked there and still is not.
  - Tests: `ten_example_keys` and `risk_and_kind_tables` are extended; `text_ref_tools_edit_their_ref_paths` and `written_paths_cover_text_ref_tools` are new.
- **Declined, as known limits:**
  - `Depth::Symbol` is measured from a stale index when the target symbol lies inside a moved source. Niche.
  - Hard links and case-insensitive spellings can defeat same-file detection. Unlikely.
  - Identifiers in anchors are ASCII only.

### D. New tests (R2)
- **tools `textref::tests`:**
  - `clips_have_an_engine_lru_cap_and_go_with_their_session`
  - `line_anchors_are_exact_and_the_rest_are_symbols`
  - `commit_refuses_stale_files_and_rolls_back_created_ones`: the "changed on disk while this call ran" refusal, and rollback removing a created file.
  - The snapshot cap test now also shows that tiny rows count.
  - `RESTORE FAILED` cannot be forced without root and is not tested.
- **tools `text::tests`:**
  - `snapshots_and_clips_do_not_cross_sessions`: covers "unknown even while the file is unchanged".
  - `an_insertion_inside_the_named_lines_refuses`
  - `refusals_cap_context_at_40_lines_and_say_when_nothing_is_left`: 40-line cap, `[11 more lines]`, "(no lines there now)", the 200-char cut.
  - `mapping_works_on_crlf_bom_and_no_final_newline_files`
  - `edit_range_argument_errors_and_symbol_delete_at_eof`: range+old, all+range, start/end, a read_file header in `new`, a symbol delete at EOF.
  - `match_rules_cover_openers_edges_and_outdent`: `(`, `[` and Python `:` openers, no prev, column 0, `"-1"` on paste, the 64-level and 1024-column bounds.
  - `string_flags_survive_a_clip_and_non_utf8_is_refused`
- **codeindex `syntax_hook::tests`:**
  - `rust_block_doc_attached_inner_and_four_slash_not`
  - `ts_jsdoc_and_export_const_arrow_widen`
  - `js_arrow_const_resolves`
  - `go_doc_widens_func_and_type`
  - `go_trailing_comment_is_not_next_funcs_doc`
  - `python_decorator_across_blank_line_stays_decorated`
  - `has_errors_reflects_tree`
- **rpc:** `session_call_tool_allowlist_precedes_authorize`.
- **agent:** `acp_child_seat_is_its_spawn_cwd_with_the_parents_write_root`, `written_paths_cover_text_ref_tools`.
- **theuth:** `failed_list_tools_lists_only_the_own_three`. `tools_proxy_to_the_engine` now also checks that an array-valued `edits` argument passes through unchanged.
- **cells:** `text_ref_tools_edit_their_ref_paths`.
- **Not added:** JS backslash-continued strings and an over-4 MiB source with a symbol ref (not in the R2 list).

### Gates (R2, final tree)
Every Rust command ran with `BUILD_LABEL=193 build-lock mbx …`, except the bare test runs. Logs are in `/tmp/193/r2*`.
- **fmt**: `cargo fmt` on tools, codeindex, agent, rpc, theuth, double, acp and cells. Main is not rustfmt-clean, so fmt's changes to 23 untouched rpc files were reverted, and `api.rs` was restored and re-patched by hand.
- **check**: `mbx check -p tools -p codeindex -p agent -p rpc -p theuth -p double -p acp -p cells --tests` gives 0 errors. The 1 warning is already on main (`codeindex/src/map/export.rs:627`).
- **clippy**, `--no-deps --tests -D warnings`:
  - `tools`: clean.
  - `codeindex`: fails only on that pre-existing `export.rs:627` import.
  - `agent`, `rpc`, `theuth`, `double`, `acp`, `cells`, with `-A clippy::chunks_exact_to_as_chunks -A deprecated`: exit 0.
- **tests, tools and codeindex**: `--no-run`, then bare `mbx test -p tools -p codeindex --no-fail-fast`.
  - `tools` lib: 267 passed, 1 failed. `pty::tests::ready_port` is a port flake that passes alone.
  - `codeindex`:
    - lib: 102 passed, 1 ignored.
    - `codemap`: 7 passed.
    - `locate`: 1 passed.
    - `index`: 20 passed, 3 failed. `map_and_symbol` and `impact_on_this_repository` are known reds on main. `project_profile_on_this_repository` (≤ 20 ms timing) fails under load and passes alone.
  - The new tests ran a second time: 27 tools (`text::`, `textref::`) and 18 codeindex (`syntax_hook::`), all passed.
- **tests, other crates**: `--no-run` with `-p agent -p rpc -p theuth -p double -p acp -p cells`, then the covering tests by name, run twice. Both runs passed:

  | crate | tests | result |
  |---|---|---|
  | agent | seat, `written_paths`, `repl::` | 24 |
  | rpc | allowlist | 1 |
  | theuth | `mcp::tests` | 7 |
  | double | `ten_example_keys` | 1 |
  | acp | `risk_and_kind_tables` | 1 |
  | cells | `text_ref_tools_edit_their_ref_paths` | 1 |
- **UI (section E)**:
  - `mbx test -p rpc --features ts --lib --quiet export_bindings`: 10 passed. `bindings/methods.json` contains `session_call_tool`.
  - The bindings were copied into `crates/desktop/ui/src/rpc/bindings/`.
  - `bun install --frozen-lockfile` was run in `crates/desktop/ui` and in `packages/apps`.
  - `bunx tsc -b --noEmit`: 0 errors.
  - `vitest run src/rpc/api.test.ts` twice: 2/2 passed both times.
  - The generated bindings are gitignored and not committed.

## R3

R3 fixed the small findings of `review-193-r2.md` as decided in the R3 brief. Numbers are that review's.

### Fixed
1. **#1, dead `dial`:** deleted from `crates/theuth/src/mcp.rs`; its two tests call `dial_with(.., None)`. A plain `check -p theuth` has 0 warnings.
2. **#3, a failed syntax hook:** `text::block_of` no longer turns the hook's `Err` into `None`. When string protection is wanted (`indent`, `move_text`, `copy_text`, `clip`) the call is refused with the hook's own message (`the syntax parser failed on <file>: …`) plus `: multi-line strings cannot be protected`; nothing is written. No parser at all (`Ok(None)`) keeps the "no parser for this language" note.
3. **#5, parses:** `parse::parse_tree` now always returns the tree, so the hook never parses a third time. Each `SyntaxHook::analyze` call parses exactly twice: `Parsers.tree` builds the one tree that the import query, string interiors and declaration widening all use, and tree-sitter-tags' `generate_tags` parses once more internally. It takes only source bytes and exposes no tree (0.27.1), so symbols cannot come from our tree without reimplementing the tags runner. The indexer's `parse` still builds the tree only for languages with an import query.
4. **#6, clip sizes:** `Clips` stores a `ClipEntry { clip, used, size }`; `size` is measured by `put_clip` before the lock. The per-session and engine-wide sums read the stored sizes, so nothing walks clip lines under `CLIPS`.
5. **#7:** `tools_proxy_to_the_engine` sends the real `edit` shape (`path` plus `edits` with one `range` and one `old` entry) and the fake engine checks the whole argument object.
6. **#8:** new `text::tests::an_acp_childs_relative_path_resolves_in_its_own_cwd` (tools, seat level). The child is seated as `acp_session` does it (parent seat inherited, then its own cwd); a relative `read_file` and `indent` touch the child's file, not the parent's. The supervisor test's comment now points there instead of claiming that case.
7. **#9, `list_tools` timeout:** the shim's `list_tools` call is wrapped in its own 30 s `LIST_TOOLS_TIMEOUT`; on timeout it lists the shim's own three tools, as on failure. The 330 s `SESSION_REQUEST_TIMEOUT` is tied to the default `tools.approval_timeout_secs` (300 s). A user who raises that setting above 330 s reopens the late-approval race: the shim gives up on a call the engine may still run after approval.

### Declined
- **#10**, one shared paths helper across `double` (`touched_paths`), `cells` (`edited_paths`) and `agent` (`written_paths`): follow-up.
- **#11**, `edit` not in `files_written`: pre-existing, left as is.
- #2 and #4 were already done in R2.

### Tests (R3)
- **tools:** `text::tests::failed_syntax_hook_refuses_a_shift` (the test hook panics on `.boom`; a `.txt` keeps the no-parser note), `text::tests::an_acp_childs_relative_path_resolves_in_its_own_cwd`.
- **theuth:** `tools_proxy_to_the_engine` changed (#7).
- No test for the 30 s `list_tools` timeout: it needs a 30 s hang; it shares the failure fallback that `failed_list_tools_lists_only_the_own_three` covers.

### Gates (R3, final tree)
Every Rust command ran with `BUILD_LABEL=193r3 build-lock mbx …`, except the bare test runs. Logs are in `/tmp/193r3/`.
- **fmt:** `cargo fmt -p tools -p codeindex -p theuth`; `git status` showed only the intended files.
- **check** `-p tools -p codeindex -p theuth`: 0 errors, 0 warnings. With `--tests`: 0 errors; 1 warning, the pre-existing `codeindex/src/map/export.rs:627`.
- **clippy** `--no-deps -D warnings`: `tools --tests` clean; `codeindex` clean (with `--tests` it fails only on that pre-existing import); `theuth --tests` with `-A clippy::chunks_exact_to_as_chunks -A deprecated` clean.
- **tests:** `--no-run`, then bare `mbx test -p tools -p codeindex -p theuth --no-fail-fast`:
  - `tools` lib: 270 passed, 3 ignored; `fd_count`: 1 passed.
  - `codeindex` lib: 102 passed, 1 ignored; `codemap` 7, `locate` 1; `index`: 21 passed, 2 failed (`map_and_symbol`, `impact_on_this_repository`, known on main).
  - `theuth` (`t`): 13 passed.
  - Second run of `text::`, `textref::`, `syntax_hook::`, `mcp::tests`: 60 tools, 18 codeindex, 7 theuth, all passed.
