# 136 F6 — as-built: the seven P2 defects

Tree `~/Develop/theuth-136`, branch `feat/136-dreaming`. Base df0521bf (F5, fully gated). P2's report is in the tree as `docs/design/136-as-built-P2.md` (54c94be6). One commit per defect, each with a test; rulings as the brief gave them.

## Commits

| sha | what |
|---|---|
| 54c94be6 | `136 F6: as-built P2 (re-proof)` — `/tmp/136/p2-as-built.md` copied in |
| 47ebb7b9 | D1 — `knowledge_retitle` refuses a title over `TITLE_MAX` |
| 9ac6116e | D2 — `knowledge_verify ok` needs a path or URL; `session:`/`meeting:` only in addition and already cited |
| 57112134 | D3 — close gate re-arms on reopen; `Skipped` parsed; the second close checks it |
| e1906e3b | D4 — the dreaming session never harvests itself |
| 575e26af | D5 — `skill_learned_edit` takes `stage` (alias `pass`); skill recipes say `stage` |
| 06d0165a | D6 — disable switches the routine off in place and removes the sensor; design §8 amended |
| ade19d50 | D7 — skill verify recipe and tool table carry `evidence` |
| fa82914d | fmt (rustfmt's wrapping of four spots above; no behaviour) |

## D1–D7

| # | fix | where | test |
|---|---|---|---|
| D1 | `Knowledge::retitle` → `ensure!(title.chars().count() <= harvest::TITLE_MAX, "retitle: title is {n} chars, the cap is 120: choose a shorter title (the rest belongs in summary)")`; nothing clipped, nothing written. `split_title` stays for the harvest path. | `crates/knowledge/src/store.rs:2041-2046` | `store.rs` `consolidation_ops…` (the retitle block, :3089-3095): 121 chars refused with that text and the file unchanged; exactly 120 accepted |
| D2 | `check_evidence(outcome, evidence, cited, data_dir)`: `ok` needs ≥ 1 `is_read_source` item (existing path, relative to the data dir or absolute, or http(s) URL) — else `verify ok needs a read: …`; every `session:`/`meeting:` item must already be in the concept's own `evidence` (`cited`, read via `Knowledge::get` before the write) — else ``verify ok: `<item>` is not in the concept's evidence — …``. `stale` keeps the one-of-three rule and its old error text. Arg doc and tool description say so. | `crates/knowledge/src/tools.rs:658-707` (`is_read_source`, `is_ref`, `check_evidence`), `:726-729` | `tools.rs` `verify_needs_evidence_from_a_read` (:1070): a cited id alone, an uncited id + `meeting:`, bare uuid, ftp, missing path → refused; path + uncited id → refused naming the id; path, URL, URL + cited id → ok; `stale` with a bare `meeting:` → ok, with `ftp://` → refused |
| D3 | `BeginNight` reopen drops `nudge: ` **and** `skipped: ` phases (the last window's, about its lists). `left_undone` now returns `Vec<(stem, line)>`; after a nudge, `Commit{journal}` closes only when it is empty or every list's stem (`retitle`/`merge`/`archive`/`verif`/`case`) appears in a `skipped: ` phase (case-insensitive), else `still not closing — no `Skipped` line for: <lists>`. `dreaming_journal(section="Skipped")` passes the whole markdown (not the first line) and the phase keeps 600 chars. Skill §7 says "one line per list, naming it". | `crates/dreaming/src/night.rs:873-880` (reopen), `:935-967` (gate), `:1003-1027` (`lines`, `not_skipped`), `:1029-1096` (`left_undone`); `crates/dreaming/src/tools.rs:684-689`; `skill.md:128` | `night.rs` `left_undone_names_the_unworked_lists_and_is_none_when_worked` (:2161, `not_skipped` coverage by stem) and `close_gate_needs_skipped_lines_and_rearms_on_reopen` (:2256): nudge → refused again naming both lists → one `Skipped` line → refused naming the other → second line → closed; reopen → phases clean, nudge again, lists worked → closed |
| D4 | `Turn::harvest_store()` = `knowledge()` minus `is_consolidating_session(self.shared.id)`; the turn-end trigger (and its `turns_since_harvest += 1`), the compaction pre-harvest and `detach_harvest` (`/knowledge harvest` → `the dreaming session does not harvest itself`) all go through it. The hint path keeps its own guard. | `crates/agent/src/loop_.rs:3202-3210`, `:5007`, `:6448-6454`, `:6618` | `loop_/harvest_tests.rs` `harvest_runs_detached_from_the_turn` tail (:130-158): session registered → due turn fires no `Harvesting`, count stays `every - 1`; `harvest_now` refused by name; `/compact cut` emits `Compacted` with no `Harvesting` |
| D5 | `SkillLearnedEditArgs.stage: Option<String>` with `#[serde(default, alias = "pass")]`; error `stage {other:?}: candidate \| record`; `retire needs stage: record`; tool description, `replay_case` description, skill lines 85/87/105 and the tool table say `stage`. Tree grep: no `pass=`/`"pass"` left in dreaming prose (`pass` remains only as the run mode). | `crates/dreaming/src/tools.rs:387-392`, `:424-427`, `:473`; `edits.rs:465`; `skill.md:85,87,105,138` | `tools.rs` `skill_learned_edit_stage_replaces_the_pass_keyword` (:999): `stage` and `pass` both deserialise; the JSON schema has `stage` and no `pass`; the skill has no `pass=`, two `stage="record"`, `{skill, edits, night, stage?}` |
| D6 | `remove_files` deleted. `disable_files`: `schedules/dreaming.md` rewritten with `enabled: false` as its first front-matter line (`night::set_routine_enabled`, shared with the routine-proposal writer), `sensors/dreaming.py` removed, `skills/dreaming.md` left. `start` with `dreaming.enabled: false` and `SetSettings(enabled: false)` call it. Re-enable: `install_files` rewrites the sensor and, when the sensor was absent (only `disable_files` removes it) and the routine is off, sets `enabled: true` — a hand toggle with the sensor present still survives (R-47). Design §8 and §9.2 amended in one sentence each. | `crates/dreaming/src/lib.rs:148-206`, `:230-241`; `night.rs:296-306`, `:1393-1399` | `lib.rs` `disabling_switches_the_routine_off_in_place_and_drops_the_sensor` (:449): after `disable_files` the routine parses with `enabled: false`, `when` intact, sensor gone, skill present, idempotent; `install_files` → `[sensor, routine]`, `enabled: true`; hand toggle → kept; empty dir → no-op. `night.rs` `set_settings_patches_the_routine_model_and_toggles_the_files` (:2640-2654) covers the actor path |
| D7 | Verify recipe: `knowledge_verify(id, "ok", evidence=[<the path or URL you read in this cell>])`, `knowledge_verify(id, "stale", evidence=[<what you read>], note=…)`, "**No read in this cell → do not verify; mark unverifiable.**" + the D2 rule; tool table `{id, outcome: ok\|stale, evidence: [path \| url (+ session:/meeting: only in addition)], note?}`. | `crates/dreaming/src/skill.md:51`, `:147` | `lib.rs` `skill_carries_the_judge_prompt_verbatim_and_stays_under_24kb` (size cap) — prose, no behaviour test; the gate itself is D2's |

Skill size: the additions (D3/D5/D7) pushed `skill.md` past the 24 KB cap (25,187 B); seven explanatory parentheticals and one sentence were shortened (sensor description, receipts sentence, backlog window note, `family` note, "only model cost of triage", Tidy-triage example, `git log` aside) → 24,555 B. No recipe or rule removed.

## §8 amendment (R-13)

`docs/design/136-dreaming.md` §8 "Settings › Dreaming": `dreaming.enabled: false` "switches the routine file off in place (`enabled: false` — as built after P2 D6: a deleted file would be projected back by the `schedules/` replica mirror at the next start), removes the sensor, the prompt block and the signals writer, and leaves the skill"; §9.2 config comment likewise. Reason: `replica::files` treats a row whose file it has never seen on disk as to-be-projected, so a delete at plugin start (before the mirror's first scan) came back five seconds later (P2 check 9).

## Gate (all through `build-lock`, label `136-F6`, `CARGO_TARGET_DIR=…/theuth-136-target/C`, jobs 4, incremental off)

| step | result |
|---|---|
| `cargo test -p knowledge -p dreaming --lib` | knowledge 61 passed · dreaming 55 passed |
| `cargo test -p agent --lib` | 505 passed, 2 ignored (70 s) |
| `cargo clippy -p knowledge -p agent -p dreaming --tests -- -D warnings` | clean |
| `cargo fmt -p knowledge -p agent -p dreaming -- --check` | clean (after fa82914d) |
| RPC / TS types | unchanged (no `just ts-desktop` needed) |
| `BUILD_SLOTS=1 cargo build -p theuth` | `…/C/debug/t` (2.12.3) built in 1 m 28 s; `t` + `theuth-documents-59.5` copied to `/tmp/136/bin2/` |
| Smoke (D6, that binary) | scratch data dir with a 3-line routine, a sensor and a skill, `dreaming.enabled: false`, `--headless`: log `dreaming disabled by config; routine switched off (enabled: false), sensor removed`; after `replica::files: skills … / schedules … replicates` the routine still reads `---\nenabled: false\nevery: 30m when dreaming.py…`, `sensors/dreaming.py` absent, `skills/dreaming.md` present; engine stopped, dir removed |

## Open

- D3 drops `skipped: ` phases on reopen as well as `nudge: ` (the brief named only the nudge): a second window's lists differ from the first's, so the old lines would have satisfied the new gate without a fresh nudge.
- D6's "disabled by us" signal is the missing sensor file; a user who deletes the sensor by hand and toggles the routine off will see it turned back on at the next start with `dreaming.enabled: true`. Benign; a state flag would be the upgrade if it ever matters.
