# 201 — Shaper audit: what the dictation shaper did to speech-to-text output

Status: built on `feat/stt-shaper-mlx` (2026-11-21). Scope: `crates/voice/src/shape/**`, the packaged-dictionary
gate in `crates/voice/src/dictionary/mod.rs`, and the global off switch `voice.shape.enabled`.

## 1. Where the shaper runs (153 §4.6)

| Path | Shaper? | Preset | Pieces | `prev` |
|---|---|---|---|---|
| Dictation, every surface (composer, TUI, companion, phone, any-app) | yes, `dictation.rs` `Task::texts` → `ShaperOwned::shape` | `voice.shape.<surface>` (default `full`) | every rolling piece **and** the final, through the utterance's `EndParser` | the shaped finals of this dictation joined by spaces |
| Talk / voice-agent input (`crates/talk`) | no — the talk leases the `Utterer` (`Dictation::lease`) and reads its text raw | — | — | — |
| Meetings | no shaper call (no `Shaper` use outside `dictation.rs`) | — | — | — |

So the damage is limited to dictation, but that is the path used every day (composer + phone in the history).

## 2. Evidence: `~/.theuth/voice.db`, `dictations` table

65 dictations (39 composer, 24 phone, 1 companion, 1 any-app). 10 have `shaped != raw`; **all 10 are damage**, none is a
wanted rewrite. 7 corrections were recorded; one is the owner undoing `Agent` → `agent`.

| # | Defect | Real example (raw → shaped) | Expected | Stage | Fix |
|---|---|---|---|---|---|
| 1 | Packaged term joins ordinary words into a lowercase package name | "there are a lot of bugs" → "alot of bugs"; "did go to my horse" → "goto my horse" (×2 rows); "this arch Linux system" → "archlinux system"; "what's up dog" → "what's updog" | unchanged | 7 dictionary (`Dictionary::lookup_scoped`, packaged layer: `norm` drops spaces so a 2-word window matches a 1-word canonical) | packaged entries may join a multi-word window only into a curated or mixed-case canonical ("git hub" → GitHub) |
| 2 | Packaged Title-case app name from two words | "I want it to be an extended display" → "to Bean extended display" | unchanged | 7 dictionary (Homebrew cask "Bean") | same gate (Title case is not informative casing) |
| 3 | Packaged canonical whose punctuation `norm` strips | "Send that to the other agent" → "other Agent!"; "the agent understands" → "the Agent! understands" (5 occurrences in 3 rows) | unchanged | 7 dictionary (cask "Agent!") | rejected: not a recase, not a multi-word join |
| 4 | Curated single-word respelling of an ordinary word | "Settings per node from a device" → "per Node.js from a device" | unchanged | 7 dictionary (curated spoken form `node`) | packaged single-word respellings no longer fire (also covers `jason`→JSON, `jupiter`→Jupyter, `grok`→Groq, `onyx`→ONNX); user/learned entries still can |
| 5 | Lone number word converted in prose | "a few one or two of them" → "one or 2 of them" | unchanged | 5 numbers (only a lone "one" was exempt) | every lone number word stays unless after "number" or beside a spoken symbol, "point" or "percent"; multi-word cardinals ("twenty three") still convert |
| 6 | `Preset::Off` was not the model's text | Off still ran tokenize → render, which respaces/recases around the model's own punctuation | byte-identical | `Ctx::run` | Off returns `raw` verbatim before tokenizing |

Not reproduced in the history (no row shows them, so no change made): spoken punctuation / "new line" / "tab" eaten mid-sentence,
end commands or near misses firing on ordinary endings, text dropped at an end command, rolling-vs-final drift. The model already
emits digits and punctuation ("120 seconds" arrived as digits), which is why every number/dictionary rewrite in the data was harmful.

## 3. The switch: `voice.shape.enabled`

`ShapePresets.enabled` (`#[serde(default)]`, default `true`; an old config without the key keeps shaping). `false`:
`ShapePresets::get` returns `Preset::Off` for every surface, so `ShaperOwned` gets the all-off `ShapeConfig` and `Ctx::run`
returns the model's text byte for byte — no stages, no end commands (the `EndParser` is never fed, so no near misses), no terminal
spacing. Corrections are not learned into the dictionary while it is off (`Message<Correction>`). Set by `set_shape_enabled(bool)`
(writes `voice.shape.enabled`, tells the actor `Setting::ShapeEnabled`), read back in `voice_settings().shape.enabled`; desktop
Settings › Voice › Dictation › Shaping has the "Shape dictated text" switch, which also disables the per-surface selects.

## 4. Tests (`crates/voice/src/shape/tests.rs`)

`history_prose_is_unchanged_with_the_packaged_dictionary` (rows 1–5 above against the real packaged set, plus "git hub actions" →
"GitHub actions" still rewritten), `lone_number_words_stay_prose`, `disabled_shaper_passes_the_model_text_through`.
