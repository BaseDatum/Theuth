# 153 as-built — C2 ENGINE NATIVE-APP TOOLS + COMMAND PROFILE (`applescript`, `shortcuts`, `powershell`, `[command, N s]` prompt block, `native-apps.md`)

Design: `153-voice-anywhere.md` §10.2, §11, §14.2, §16 row 19. Worktree `feat/153-c2` off `integrate/153` `e464a304`.

## Files
- `crates/computer/src/native.rs` (new): `run(program, args, stdin, timeout_secs) -> Result<Run, String>` (tokio `Command`, argv only, no shell,
  stdin null unless fed, `kill_on_drop` + `tokio::time::timeout` → `"<prog>: timed out after N s"`), `truncate` (16 KiB per stream, char-safe,
  `…(N more bytes)`), `output(prefix, run) -> ToolOutput` (§11.3 text: stdout, `prefix: stderr`, trailer `exit N · T s`; `is_error` on non-zero;
  `structured` = `{ok, stdout, stderr, exit_code, ms}`, `RETURNS = "dict"`), `AppleScriptTool` (`osascript -e <script>`, macOS),
  `ShortcutsTool` (`shortcuts run <name> [-i -] -o -`, `name: "__list__"` → `shortcuts list`, macOS), `PowerShellTool`
  (`pwsh -NoProfile -NonInteractive -Command <script>`, falls back to `powershell` when `pwsh` is missing, Windows), `native::all()`.
  Every Args struct: `timeout_secs: Option<u64>` (default 30, clamp 1..=120) + `purpose: Option<String>`.
- `crates/computer/src/tools.rs`: `all()` appends `native::all()`; `PROMPT` gains `## Native apps` and `## Command mode` (§10.2 text).
- `crates/computer/src/lib.rs`: `pub mod native;`, `pub const NATIVE_APPS_SKILL = include_str!("../native-apps.md")`.
- `crates/computer/native-apps.md` (new, 116 lines): the §11.4 skill — how-to rules, Reminders/Notes/Calendar/Messages/Mail/Music/Finder/
  Shortcuts recipes, Outlook COM / Settings / Explorer / Notepad / BurntToast on Windows, safety rules. Written from the apps' dictionaries.
- `crates/theuth/src/main.rs`: `first_run` writes `skills/native-apps.md` (binary-owned, rewritten on change like `decide.md`).

## Design said vs built
- §10.2 `purpose`: the three native tools take an explicit `purpose: Option<String>` arg (in the schema, so the model sees it). Every other tool
  gets it as an extra keyword argument: the runtime stubs are `(…, **extra: Any)` and `repl::merge_args` passes unknown kwargs through into
  the `ToolCall.args` the card reads from `ToolCallStarted`; serde ignores unknown fields on typed Args (no `deny_unknown_fields` in `tools`/`computer`).
  The prompt block says so with a `shell(command=…, purpose="executing: …")` example. No engine change beyond the prompt, as the design says.
- §11.1: built as specified; `shortcuts` adds `-o -` so a shortcut's output lands on stdout (without it `shortcuts run` prints nothing).
- §11.3 + brief's `{ok, stdout, stderr, exit_code, ms}`: both — the text is the §11.3 form, the dict is `structured`.
- §11.2 reclassifier (`do shell script`, `rm `, … → Destructive) and `computer.native: off|mutating|destructive`: **not built.** The registry's only
  permission input is `spec().risk` (`crates/tools/src/lib.rs:365`); a per-call risk needs a new seam through `registry.rs` `gate`/`inspect`/
  `admit_app`. The default policy is `Ask` for both `mutating` and `destructive`, so the first run asks either way; the skill's safety rules
  carry the confirm-before-delete/send instruction. Add the seam when a policy that allows `mutating` but asks for `destructive` is wanted.
- §11.5 `policy_asks_for_mutating_native_tool` lives in `computer::native::tests` (asserts `Risk::Mutating` + `Policy::default().action == Ask`
  + the `purpose` key in every schema); the registry harness is private to `crates/tools`, which cannot depend on `computer`.

## Tests (crates/computer)
`native::registers_per_platform`, `native::policy_asks_for_mutating_native_tool`, `native::output_truncated`, `native::output_shape_and_trailer`,
`native::timeout_is_clamped`, `native::exec_without_shell` (`/bin/echo` gets `x; rm -f M; echo pwned > M` as one argv entry, prints it, M never
appears), `native::timeout_kills` (`/bin/sleep 10` with 1 s), `native::stdin_is_fed_and_nonzero_exit_is_not_ok`,
`native::applescript_returns_result_line` (live `osascript`, `return 1 + 1` → `2\nexit 0 · …`; `return "x"; echo pwned` → syntax error, no app),
`tools::all_registers_computer_tools_then_native_with_their_risks` (named `…_with_screenshot_read_only` until 161), `tools::prompt_block_mentions_command_marker`.

## Gates
- `build-lock mbx clippy -p computer -p tools --tests` → `Finished dev profile … 18.02s`, 0 warnings.
- `build-lock mbx test -p computer --no-run` → exit 0; bare `MBX_SCHEDULER_TESTS=0 mbx test -p computer --lib` →
  `test result: ok. 63 passed; 0 failed; 0 ignored` (14 of them the native/tools tests above, incl. the live `osascript` smoke).
- `build-lock mbx check -p computer -p tools -p theuth --tests` → **fails upstream** in `crates/voice/src/dictation.rs:943,988`
  (`suggest` takes `Vec<Correction>`, `history::answer` wants `&mut Dictionary`; last touched by `18d8eeef 153-A4`, not by this branch).
  `computer` and `tools` do not depend on `voice`; the `theuth` row in `first_run` is a one-tuple addition mirroring `decide.md`
  and is unverified by compile until `voice` builds again.

## Hand-offs
- **C1 (command card):** `purpose` is a top-level key in `ToolCallStarted.arguments` for every tool call the model makes under the profile —
  explicit on `applescript`/`shortcuts`/`powershell`, an extra kwarg elsewhere; the prefix is `checking:` / `executing:` / `verifying:`;
  absent → fall back to the tool name. Native tool results: `is_error` on non-zero exit or timeout; text ends with `exit N · T s`.
- **Windows session:** `powershell { script, timeout_secs?, purpose? }` → `pwsh -NoProfile -NonInteractive -Command <script>`, else `powershell`
  with the same args; `Risk::Mutating`; dict `{ok, stdout, stderr, exit_code, ms}`; untested on Windows here (`#[cfg(windows)]`, compiles only
  on that target — run `mbx test -p computer native::` there and add `powershell_returns_stdout`).
