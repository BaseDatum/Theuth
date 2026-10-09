# 153 as-built — A3 (history store + correction learning)

## Files
- `crates/voice/migrations/0001_voice.sql` — §6.5 schema verbatim + one column (below).
- `crates/voice/src/history/mod.rs` — `Store` (sqlx SqlitePool, WAL, `migrate!("./migrations")` on open, pattern of
  `crates/artifact/src/store.rs:627-640`), audio files, budget, suggestion flow, `answer`.
- `crates/voice/src/history/learn.rs` — pure policy: `candidates`, `due`, `occurrences`, the §6.2 constants.
- No `audio.rs`/`store.rs` split: the store is ~600 lines with tests; one module is enough.

## Design vs built
| Design / brief | Built |
|---|---|
| §6.5 schema | verbatim, plus `dictations.utterance_ends TEXT NOT NULL DEFAULT ''` — comma-separated cumulative byte end offsets per utterance in the `.pcm16` file, so `audio(id, Some(u))` can slice one utterance. §6.5 had no per-utterance offsets. |
| `attach_audio(id, utterance, pcm)` (frozen) | kept; the brief's `AudioWriter { push, finish }` is not needed — `Store` is `Clone` and `attach_audio` appends per chunk. Audio may arrive before the row exists (file + in-memory offsets; the row picks them up at `record`); a later `record` never overwrites the audio columns. Per dictation capped at `PCM_RETENTION_BYTES` (120 s). |
| `correction(id, sent_text) -> Vec<Correction>` (frozen) | **deviation**: returns `Vec<Suggestion { id: SuggestionId, heard, typed, occurrences, surface, has_audio }>` — the exact payload of `VoiceEvent::DictionarySuggestion`. `Correction` has no `SuggestionId`, so the frozen return type could not be published or answered. |
| `answer(id, answer)` (frozen) | **deviation**: `answer(id, answer, dict: &mut Dictionary)` — `Add` calls A2's `Dictionary::put` (learned `Entry`, `Source::Learned`, `Scope::Global`, heard variant with empty metaphone; A2's `put` is expected to compute codes), `Never` calls `Dictionary::negative` and writes `suggestion_state.never`, `NotNow` writes a 7-day cooldown. The store does not own the dictionary; the actor owns both. |
| `timings(id, DictationTimings)` (brief) | not built: `HistoryRow` already carries `press_ms/first_partial_ms/first_final_ms/final_ms`; A4 fills them in the row it records at `Finished`. |
| budget constants | `HISTORY_ROWS = 200`, `AUDIO_BUDGET = 64 MiB` as defaults; `Store::with_budget(rows, audio_bytes)` applies `voice.history.rows/audio_mib`. Enforced after every finished `record`: audio first (oldest `audio_path` unlinked, row kept with `audio_path NULL, audio_bytes 0`; the newest dictation is exempt), then rows beyond the cap deleted (corrections cascade; `foreign_keys` is sqlx's default ON). |
| privacy | `Store::recording_allowed(&HistoryConfig, private_session) -> bool`; the actor skips `record`/`attach_audio`/`correction` when false. |
| `IGNORES_PER_SESSION` | a published card unanswered for `CARD_TIMEOUT_MS` (20 s, §6.4's auto-dismiss) counts as ignored at the next `correction`; 3 ignores stop cards until restart. The same pair is not re-suggested while its card is pending. |
| `due(heard, typed, history, now)` (frozen) | pure `SUGGEST_AFTER` check only (2 in 7 days, heard case-insensitive, typed exact); cooldown/never/ignores are store state and checked in `correction`. |
| `Correction.utterance` | always `0`: the delivered text is the joined finals, no utterance boundaries. The card should play `dictation_audio_get(id, None)` (whole dictation) — hand-off to the card owners. |
| candidates | whitespace tokens, LCS alignment; only 1↔1 replacement runs are candidates; a replacement adjacent to inserted/deleted text is ambiguous and skipped. `INFLECTIONS` compared case-insensitively, `y → ies` included. |

## Tests (`mbx test -p voice history::`)
- `learn::tests::candidates_single_word_non_inflection` (28-row table incl. rejects), `learn::tests::due_after_2_in_7_days`
- `tests::round_trip` (migrate on fresh dir, record/list/audio per utterance/forget one/forget all, reopen)
- `tests::budget_evicts_audio_first_then_rows`, `tests::audio_capped_at_retention`
- `tests::suggestion_flow_cooldown_never_ignores`, `tests::recording_allowed_respects_privacy`
- sqlx: runtime strings (`sqlx::query`), no `query!` macros, no `.sqlx/` offline data, no `DATABASE_URL`.

## Gates
See the DONE message for the summary lines (clippy `-p voice --tests`, `test -p voice --no-run` + bare run).

## Hand-offs
- A4: `Store::open(data_dir).await?.with_budget(cfg.rows, (cfg.audio_mib as u64) << 20)` at startup; `attach_audio(id, utterance, &i16_chunk)`
  while dictating (convert f32 → i16); `record(HistoryRow { finished_ms: Some(..), .. })` at `Finished` (timings in the row); on
  `dictation_correction` → `for s in store.correction(id, &sent).await? { publish DictionarySuggestion { id: s.id, heard: s.heard, .. } }`
  (skip when `!cfg.learn`); `dictionary_suggestion_answer` → `store.answer(id, answer, &mut dictionary).await`. All gated by
  `Store::recording_allowed(&cfg.history, session.is_private)`.
- A2: `Dictionary::put` receives learned entries with empty `metaphone`; compute codes in `put`.
- Integrator: the two signature deviations above (`correction` return type, `answer`'s `dict` param).
