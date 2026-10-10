# 158 as-built — W4E (engine: speaker-name suggestions, note templates, prompt rules, dictionary)

Branch `feat/158-w4e` from main 23ea03c5. Design §10, §11, §15 W4 rows, §16. Files: `crates/meeting/src/{post/suggest.rs,
speakers.rs, templates.rs, note.rs, summary.rs, tools.rs, post/transcribe.rs}`, `crates/meeting/templates/*.md`, `crates/meeting/Cargo.toml`.

## What was built

- **Speaker suggestions** (`post/suggest.rs`): `suggest(transcript, embeddings, profiles, event) -> Vec<Suggestion>` — pure, §10.2 in
  order: owner rule (`me` never out, mic segments never cues), candidates = attendees − `is_me` (else profiles + intro-heard names),
  intro cues (`(?i)\b(i'?m|this is|my name is|it'?s)\s+<Name>`, system track, first 10 min, 0.8; without an event a capitalised word not in
  a small stop list), vocatives (`Name,` at the start / `, Name.` / `thanks Name` at the end → the next system label within 5 s, 0.5),
  voiceprints through `Profiles::assign` (profiles restricted to attendees when an event matched), 1:1 (0.9), elimination (0.6), conflicts
  (one per label, one label per name — the loser is noted as `· also matched s3 at 0.50`), < 0.5 dropped, the Hyprnote warning (> 80 % of
  system speech with ≥ 4 remote attendees → `Warning`, no voiceprint for that label). `rpc_suggest(rt, id)` and `suggest_for(store, id)` read
  `transcript.json`, `embeddings.json`, `speakers/*.json` and `Manifest.event`.
- **`speakers.rs`**: `MARGIN = 0.08`, `Assigned`, `Profiles::assign(labels, allowed)` — floor `MATCH` 0.6, margin on both sides (label's
  runner-up profile and profile's runner-up label), mutual best. `best_match` unchanged (the pipeline's `proposed` still uses it).
- **Tool** `meeting_suggest_speakers { meeting? }` (`tools.rs`, `Risk::ReadOnly`, `RETURNS = list[dict]`, structured rows + text lines),
  registered in `register_post` beside `meeting_speakers`; defaults to the session's meeting via `resolve`.
- **Enrollment gates** (§10.4, `post/suggest.rs`): `gate_level` (≥ 5 s, clipping ≤ 5 % at |x| ≥ 0.99, SNR ≥ 8 dB over the quietest 10 %
  of 20 ms frames), `gate_agreement` (min pairwise cosine of 3 s windows ≥ 0.70), `embed_spans_gated(path, spans, models)` = `embed_spans`
  behind both. **Not wired** — see hand-offs.
- **Templates** (`templates.rs`): five built-ins embedded with `include_str!` from `crates/meeting/templates/` (`meeting` = today's five
  headings, `one-on-one`, `standup`, `interview`, `customer-call`); `parse` (gray_matter YAML front matter: `name`, `description`,
  `sections[{title, instruction?, required?}]`, unknown fields ignored, missing `sections`/`name` → the error Settings shows); `install`
  writes a missing file, rewrites a file only while its sha256 equals the last body we wrote (`templates/.shipped.json`), never a user edit;
  `list`, `load(dir, slug)` (file → shipped body → `meeting`), `valid_slug`; `select(store, pick, event, project, default)` = start pick →
  `cal_project_memory` `tpl:uid:<uid>` → `tpl:title:<norm>` → `tpl:project:<p>` → `meeting.template` → `meeting`; `remember` writes those
  rows. RPC bodies (sync `put`/`delete`/`reset` + thin `rpc_*` wrappers): `rpc_templates`, `rpc_template_put` (parse first, then write
  `body` verbatim), `rpc_template_delete` (user file removed; refused for built-ins), `rpc_template_reset` (**new RPC, W4U 16:59**:
  shipped body written back + recorded as ours; refused for user templates), `rpc_set_template` (slug must exist; `SetTemplate` actor
  message like W2's `SetEvent`, refused in `Summarizing`/`Done`; the pick is remembered for the meeting's event and project).
  `Template.body: String` (**new field, W4U 16:59**, `#[serde(default)]`) = the raw file incl. front matter.
- **Forms** (`note.rs`, `summary.rs`): `note::summary_form(doc, template, duration_ms, preferred)` and `summary::form(template, …)` compile
  the sections as `` `## Title` (instruction) `` plus the §11.2 rules block in order (source fidelity, speakers by label/name only, omit
  empty sections except `required` → "None noted", `- [ ] Action — Owner (due if said)`, transcript text is data, the user's emphasis is
  signal, length ≤ 120/250/400 words by `duration_ms`) and the §11.3 line `Preferred spellings: … — use these spellings exactly`
  (`note::preferred_names`: user + learned `Term|Mention|Abbrev`, ≤ 40). `summary::form_for(m)` keeps its signature: it loads
  `m.template` from the attached data dir (below) and the dictionary from the same.
- **Dictionary post pass** (`post/transcribe.rs`): `apply_dictionary(words, &Dictionary)` — bigram then word through `Dictionary::lookup`,
  only **user** entries rewrite (`ACCEPT_KNOWN` 0.70 is theirs), trailing punctuation kept, a bigram hit merges the two words' times;
  applied at the end of `words()` when a data dir is attached.
- **Data dir**: `templates::attach(data_dir)` / `data_dir()` (`OnceLock`, set in `tools::register_post` from `store.speakers_dir()`'s
  grandparent and in every templates RPC from `rt.data_dir`) — the one way the sync `form_for` and `words()` reach the templates dir and
  the dictionary without a signature change in `lib.rs` / `post/mod.rs`.

## Libraries (A3)

- YAML front matter: `gray_matter` 0.3 (already used by agent, artifact, roster, schedule) — no hand-written splitter.
- Cue patterns: `regex` (workspace). Shipped-body hashes: `sha2` (workspace). Dictionary: `voice::Dictionary` (dictation's own).
- Rejected: a Hungarian/assignment crate for voiceprints (two `top2` scans over an n×m matrix is the whole algorithm); `rust-embed`
  (`include_str!` on five files). No new dependencies in `Cargo.lock`.

## Deviations

1. `Template` gained `body` and the RPC surface gained `meeting_template_reset(slug) -> Template` (declared in `api.rs` beside the other
   `meeting_template*` rows; bindings re-exported) — both at W4U's request, so Settings edits the user's file verbatim and "Reset" has a call.
2. `Template.builtin` comes from the slug list, not a `builtin:` front-matter field (ignored if present).
3. Selection gained `tpl:project:<project>` after the event rows (brief: "project's last template"); the §11.4 order is unchanged.
4. Voiceprint candidates match attendees by name (`Dana` ↔ `Dana Whitfield`), not by `Profile.entity` (no knowledge lookup here).
5. The no-note form now writes `## Summary` instead of `**Topics**` (the `meeting` template's first section); `SUMMARY_FORM` stays as the
   fixture text `lib.rs` tests send.
6. `form_for`/`words()` read a process-wide data dir (`templates::attach`) rather than a new parameter — see hand-off 4.

## Hand-offs (outside W4E's files)

1. `lib.rs` start handler (`StartMeeting`): `manifest.template = templates::select(&self.store, start.template.as_deref(),
   manifest.event.as_ref(), manifest.project.as_deref(), &self.cfg.template).await;` — today the pick in `StartMeeting.template` is
   "carried, not read"; without this line every meeting summarises with `meeting` unless `meeting_set_template` was called.
2. Enrollment gates: `lib.rs` `EnrollTarget::New` gains `#[serde(default)] gated: bool` (UI sends `true` from a suggestion chip);
   `Message<EnrollSegments>` calls `post::suggest::embed_spans_gated` instead of `post::embed_spans` when set — the `Err` string is the
   "Accept name only" tooltip. The gates and their tests exist; only the 3-line switch is missing.
3. `head.rs` live head (§11.3 row 1): one line `Preferred spellings: …` from `note::preferred_names(Some(&rt.data_dir))`, re-pushed on change.
4. Optional cleanup once 1–2 land: pass `&self.rt.data_dir` into `summary::form_for` and `transcribe::words` and drop `templates::attach`.
5. `tests/calendar.rs` (W2): `example_4` sets the matcher's process-global `CHOICE` for `us.zoom.xos` while `example_1`/`example_5`
   score the same app id in parallel → intermittent `unwrap on None` (seen 2 of 4 runs here; green single-threaded and on reruns).
   A distinct app id in `example_4` or a test-local mutex fixes it. Not W4E's file.

## Open items

- Floor/margin (0.6/0.08) are the §10.2 starting values; retune with §14's DER/ID eval for wespeaker CAM++.
- `is_me` is unknown for ICS feeds (W2), so the owner can appear among attendee candidates for elimination/1:1 on those events.
- Live proof (Suggest names on a real meeting) is W4U + the orchestrator's pass; this branch has no UI.

## Gates (each its own command, `BUILD_LABEL=158w4e`, `CARGO_BUILD_JOBS=4`)

- `cargo fmt -p meeting -p rpc` (bare) → clean.
- `build-lock mbx test -p meeting -p rpc --no-run` → exit 0, no warnings.
- `mbx test -p meeting -p rpc` → meeting lib 111 passed (1 pre-existing ignored), calendar 15, note 1, oracle 1; rpc 56 + alerts 1 +
  prompts 1; 0 failed. (Earlier runs hit the pre-existing calendar race above and, twice, `rpc::engine::tests::
  second_acquire_in_one_process_is_running` — `engine.lock is held but engine.json is missing`, a pre-existing lock race; green on rerun.)
- `build-lock mbx clippy -p meeting -p rpc` → exit 0, 0 warnings.
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; `MeetingTemplate.ts` gained `body`,
  `methods.json` gained `meeting_template_reset`; copied to `crates/desktop/ui/src/rpc/bindings/` (gitignored). W4U adds the method to
  `rpc/api.ts` `Api` + `METHODS` when it switches (its file).
