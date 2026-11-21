# 204 — Voice settings: dictionary page and global polish switch (as built)

Amends 153 (§5.6, §9, §14.3). Two user-facing pieces, both on the existing engine; no new process.

## 1. `voice.polish_enabled`

- Config `voice.polish_enabled: bool`, default `true` (`VoiceConfig` is `#[serde(default)]`; old configs load as on).
- Where polish runs: only the agent supervisor's `Polish` handler (`crates/agent/src/supervisor.rs`), reached through the `polish` RPC.
  The dictation actor never polishes on its own: `DictationOpts` has no polish flag (the per-app flag is `AppProfile.polish`, read by
  the macOS companion's any-app path, which calls `polish`). The desktop UI and the TUI do not call `polish` today.
- Off: `Polish` replies `PolishReply { text: None, refused: Some("polish off"), model: None, ms: 0 }` before any target lookup or
  model call — 153 §14.2's "deliver the shaped text" form, so every client keeps its text. `Rewrite` (153 §13.6) is an explicit
  user command and is not gated.
- Live value: `Agents.polish_enabled` (read from config at start); `SetPolishEnabled` writes `voice.polish_enabled` to config.yaml
  first, then flips it. RPC `set_polish_enabled(enabled)` (talk, beside `set_shape_enabled`); `voice_settings.polish_enabled`
  reads the live value from the supervisor (`GetPolishEnabled`), like `polish_model`.
- UI: Settings › Voice › Dictation › **Polish** — first row switch **"Polish dictated text"**; off greys the "Polish model" picker
  (and with it the effort chips).

## 2. Settings › Voice › Dictionary

Its own tab (`voice-dictionary`, after Dictation; `VoiceDictionary.tsx`), like History.

- Explanation line, then toolbar: search field ("Search dictionary"), source chips **Yours** / **Learned** / **Built-in** with counts
  (Yours + Learned on by default), **Add entry**.
- Yours + Learned come from `dictionary_list` (which never returns packaged entries) and are filtered client-side, case-insensitive,
  over canonical, spoken and heard forms.
- Built-in (40,641 packaged terms) is searched on the engine: new RPC `dictionary_search(query, source, limit) -> { entries, total }`
  (substring of the matcher-normalised query in normalised canonical / spoken / heard forms; `limit` capped at 500) and
  `dictionary_counts() -> { user, learned, packaged }`. The UI queries it only with the Built-in chip on and ≥ 2 characters, at
  most 100 rows, then "Showing 100 of N — refine the search". The chip hides on an engine without `dictionary_search`.
- Rows: canonical (bold, monospace when it has non-letters) · heard-as chips (max 4, then +N) · scope (Everywhere / surface / app) ·
  source · actions: **Edit** and **Delete entry** for Yours/Learned, **Override** for Built-in (opens the form pre-filled as a new
  User entry).
- Form (inline): "Written as" (required), "Heard as" (comma-separated, lower-cased), "Applies" (Everywhere or one of the five
  surfaces; an app-scoped entry keeps its app), "Keep exact casing" → the `keep_case` tag (the matcher then leaves case-only
  differences alone and never capitalises a lowercase canonical at a sentence start). Save = `dictionary_put` with `source: user`;
  a new entry sends `id: ""` and the engine mints one. `dictionary_put` upserts by canonical + scope, so an edit that changes either
  first deletes the old id.
- Empty state: "No entries yet. Add the names and terms dictation gets wrong, or correct a dictation and choose Add when asked."

## 3. Follow-up

- "Never rewrite" (negatives): the engine has `DictionaryOp::Negative` internally but no RPC lists or edits negatives; not added.
