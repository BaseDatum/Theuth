# 156 ENGINE — as built (W3, `feat/156-engine`)

D10 (bounded `segment_wav`) and D14 (`meeting_speaker_enroll`) of `docs/design/156-meeting-view.md`. Commit `59ec7e43` on `feat/156-engine`, off the contract commit b3a1988c. Crates touched: `meeting`, `rpc`. No migration, no config keys, nothing under `crates/desktop/`.

## What was built, file by file

### `crates/meeting/src/transcript.rs`
- 18-21: `ENROLL_MIN_MS = 1_000`, `ENROLL_MAX_MS = 30_000`.
- 332-354 `speech_spans(t, segments) -> Result<(Track, Vec<(u64,u64)>), String>`: the word spans of the chosen segments **in transcript (chronological) order**, one track. Errors: `pick at least one segment`, `no segment {id} in this meeting`, `segments span both tracks`. A segment with no words contributes its own `[t0, t1)`.
- 356-390 `Voiceprint { spans, speech_ms, available_ms }` and `voiceprint_spans(spans)`: `available_ms` = sum; under 1 000 → `pick at least 1 s of speech (got {N} ms)`; otherwise the spans cut at 30 000 ms total, the last span trimmed.
- 392-432 `link_enrolled(t, segments, profile, embeddings, profiles) -> bool`: when every chosen segment carries one label, that label gets `name = profile.name`, `profile = Some(id)`, `proposed = None`; then every *other* label with `profile == None` and an entry in the embedding map gets `proposed = best_match(e)` (cosine ≥ 0.6, `None` when nothing matches any more). Returns `false` and leaves `t` untouched for mixed labels.
- Tests 682-756: one track in transcript order / two tracks / unknown id / empty; the 30 s cap trimming the last span and the 1 s floor; the link plus the proposal refresh (stale proposal replaced, linked label untouched, mixed labels no-op).

### `crates/meeting/src/speakers.rs`
- 109-135 `enroll` — behaviour unchanged (`EditOp::Enroll` still uses it); its mean and write now go through the shared `fold` (230-244) and `write` (202-211).
- 137-164 `create(name, entity, emb) -> Result<Profile, String>`: trims, refuses `empty name`, `empty embedding`, and a case-insensitive duplicate with `a speaker named {X} is already enrolled` (X = the stored name, so it equals what `meeting_speakers` lists).
- 166-175 `add_sample(id, emb) -> Result<Profile, String>`: `no speaker profile {id}` when unknown; otherwise the running mean `(mean·n + emb)/(n+1)`, samples + 1, `updated_at`, written through.
- 177-211 `push`, `get`, `write` helpers (the `.json.tmp` + rename write that `enroll` had inline).
- Test 282-313: duplicate refused case-insensitively, trimmed; `add_sample` by id ×2 → `(1/3, 2/3)`, samples 3, survives reload; unknown id and size mismatch refused.

### `crates/meeting/src/post/transcribe.rs`
- 30-68 `OPUS_RATE = 48_000` and `opus_ranges(path, ranges) -> anyhow::Result<Vec<f32>>`: `::ogg::PacketReader` over a `BufReader<File>` + `opus::Decoder` (both already dependencies of this crate). Skips `OpusHead`/`OpusTags`, decodes every packet from the start (libopus is stateful, so the samples are identical to `voice::decode`'s) but **stops at the last range's end and keeps only the samples inside the ranges**. Output is the kept samples concatenated in time order. A truncated tail ends the stream, as `voice::decode`'s `while let Ok` does. `ponytail:` note names the upgrade (granule-position page seeking).
- Test 167-202 `opus_ranges_match_the_full_decode_sliced`: a 3 s 440 Hz sine through the crate's own `opus::Encoder` + `ogg::Writer`; a range equals `voice::decode(...)` sliced sample-for-sample; two ranges concatenate; the end clamps at the track; a range past the end is empty; `[0, 60 s)` equals the full decode.

### `crates/meeting/src/post/mod.rs`
- 306-319 `pub fn embed_spans(path, spans, models)`: `opus_ranges` → `voice::resample(48k → 16k)` → `Embedder::open` → `compute(to_i16)`, the same embedder path as `me_embedding`. Blocking.

### `crates/meeting/src/lib.rs`
- 961-1008 `EnrollTarget` (`#[serde(tag = "kind")]`, `New { name, entity }` / `Existing { profile }`), `Enrolled { profile, speech_ms, available_ms, transcript }` (both exactly the Contract's derives; `ts(type = "number")` on the two `u64`s), `EnrollSegments { meeting, segments, target }`. All at the crate root.
- 1010-1017 private `EnrollApply`: the second half, asked by the enrollment task.
- 1298-1304 `live_id()`, now shared by `stored()` (1307-1312) and `enrollable()` (1314-1334). `enrollable` refuses: the live meeting and `Starting|Recording|Paused|Stopping` → `still recording: wait for the meeting to end`; `Post(_)` → `still processing: wait for the post pipeline`; `Summarizing` → `Summarizing: wait for it to finish`; `Idle|Failed` → `{state}: this meeting has no transcript`. `Review` and `Done` pass.
- 1336-1350 `track_path(manifest, track)`: the per-track file lookup `SegmentAudio` had inline, now shared.
- 3355-3384 `SegmentAudio` handler: unchanged flow, uses `track_path`.
- 3386-3407 `segment_wav`: `opus_ranges(path, &[(t0, t1)])` instead of a whole-file `voice::decode`; the WAV is the same 16-bit mono 48 kHz `[t0, t1)` (D10).
- 3409-3492 `impl Message<EnrollSegments>`: `DelegatedReply`. On the actor: `enrollable`, read the transcript, `speech_spans`, `voiceprint_spans`, load `Profiles`, refuse a bad target early (duplicate name / unknown id, so the user does not wait seconds for that), `diarize::models(..)` or `speaker models not installed under {dir}`, resolve the track file. Then `tokio::spawn` + `spawn_blocking(embed_spans)` off the mailbox, exactly as `SegmentAudio`, and `actor.ask(EnrollApply)` with the embedding.
- 3494-3528 `impl Message<EnrollApply>`: back on the actor: `enrollable` again, `Profiles::load`, `create` or `add_sample`, **re-read** `transcript.json` and `embeddings.json`, `link_enrolled`, and `store.save_transcript` (the same call `meeting_edit` uses; re-indexes) only when something was linked. Returns `Enrolled` with the saved transcript, or the unchanged current one.
- Tests: 4231-4330 `refused`, `mic_transcript`, `enroll_segments_is_gated_by_state` (Post / Summarizing / Recording refused with the exact strings; Review and Done reach `speaker models not installed under …` on a bench without models; an unknown segment id is refused). 4332-4388 `#[ignore] enroll_segments_on_a_real_meeting_copy` (below).

### `crates/rpc/src/api.rs`
- 1805-1812 trait method `meeting_speaker_enroll(id, segments: Vec<u32>, target: EnrollTarget) -> RpcResult<Enrolled>`, beside `meeting_speakers`.
- 6863-6878 impl: `send(&meeting()?, meeting::EnrollSegments { .. })`, as `meeting_segment_audio`.

### `crates/rpc/src/export.rs`
- 647-648 `meeting::EnrollTarget`, `meeting::Enrolled`.

## Emitted TS (binding; ts-rs 2026-10-09)

Type names ts-rs uses: `ProfileId`, `Profile`, `MeetingTranscript`, `MeetingId` — the ones the Contract guessed. Doc comments aside, the shapes are the Contract's lines.

`bindings/EnrollTarget.ts`:
```ts
import type { ProfileId } from "./ProfileId";
export type EnrollTarget = { "kind": "New", name: string, entity: string | null, } | { "kind": "Existing", profile: ProfileId, };
```
`bindings/Enrolled.ts`:
```ts
import type { MeetingTranscript } from "./MeetingTranscript";
import type { Profile } from "./Profile";
export type Enrolled = { profile: Profile, speech_ms: number, available_ms: number, transcript: MeetingTranscript, };
```
(The real files carry `/** … */` per field; copies at `/tmp/156/proof-engine/EnrollTarget.ts`, `Enrolled.ts`.) `bindings/methods.json` lists `meeting_speaker_enroll` between `meeting_speaker_delete` and `meeting_speakers`.

## Decisions not spelled out in the brief

1. **"Segment order" = transcript (chronological) order**, not the order of the `segments` argument. The UI's selection order is arbitrary; chronological is what "the first 30 s" means and lets the decoder make one forward pass.
2. **Empty `segments`** → `pick at least one segment` (the brief lists no string; the UI disables the action anyway).
3. **`Idle`/`Failed`** → `{state}: this meeting has no transcript`. The brief covered Live, Post, Summarizing, Review, Done only.
4. **Bad target refused before embedding** (duplicate name, unknown id) in the first half, and again by `create`/`add_sample` in the second. Saves the user a multi-second wait for a certain refusal; the second check keeps it race-safe.
5. **Duplicate-name message uses the stored name** (`a speaker named Dana is already enrolled` for a `" dana "` request), so W2's "[Add to X instead]" matches the `meeting_speakers` list by name.
6. **`voiceprint_spans` returns a `Voiceprint` struct**, not a 3-tuple (clippy `type_complexity`). The Contract names only `speech_spans`, which has its exact signature.
7. **The bounded decoder lives in `post/transcribe.rs`** (allowed "small bounded-decode helper"), not `crates/voice` (off-limits) and not `ogg.rs` (not in my file list). It decodes from packet 0 — stopping at `t1` and not retaining before `t0` is what D10 asks; seeking is the recorded upgrade path.
8. **`embed_spans` cuts at 48 kHz then resamples the concatenation**, where `me_embedding` resamples the whole track then cuts. The alternative needs the whole track in memory, which D10/D14 avoid; the real-data run below scores 0.92 against the pipeline's own `me` embedding.
9. **The real-data test exercises the blocking path + `Profiles` + `link_enrolled` directly**, not the actor: the actor needs a `Store` rooted at a data dir and the copy sits under `/tmp/156/w3-data/meetings/`. The actor-side state gating has its own test on the bench.
10. **Profile file write is not atomic with the transcript save.** If `save_transcript` fails after `create`, the profile exists and the label is unlinked; the user sees the error and can "Add to X" again. Same shape as today's `EditOp::Enroll`.

Deviations from the brief: none.

## Gates

All through `~/.theuth/bin/build-lock mbx …` with `BUILD_LABEL=156-ENGINE`, logs under `/tmp/156/proof-engine/`:

1. `mbx test -p meeting -p rpc --lib --no-run` (lock) then bare `mbx test -p meeting -p rpc --lib`: **meeting 75 passed, 0 failed, 1 ignored; rpc 56 passed** (`gate1-test.log`).
2. `mbx clippy -p meeting -p rpc --all-targets -- -D warnings`: **clean, exit 0** (`gate2-clippy.log`; two findings fixed on the way: `type_complexity` → `Voiceprint`, `collapsible_if` → let-chain).
3. `BUILD_SLOTS=1 … mbx test -p rpc --features ts --lib --quiet export_bindings`: **8 passed** (`gate3-bindings.log`); TS above.
4. `cargo fmt -p meeting -p rpc`, bare: only my hunks changed (diff checked).
5. Real-data test, once, on the copy:
   ```
   THEUTH_TEST_MEETING_DIR=/tmp/156/w3-data/meetings/01a12243-f793-7921-beb7-04de9329c73f THEUTH_DATA_DIR=~/.theuth \
     mbx test -p meeting -p rpc --lib enroll_segments_on_a_real_meeting_copy -- --ignored --nocapture
   ```
   **ok** (`gate5-realdata.log`): `segments [4, 5, 8] on Mic: 32 spans, speech_ms 9846, available_ms 9846; embedding len 512, 639 ms; profile 01a12295-… samples 2; cosine to the pipeline's "me" 0.919; linked true`. Profiles were written to a temp dir; `~/.theuth/meetings/speakers/` is still empty and the copy's `transcript.json` mtime is unchanged.

## Proof files (`/tmp/156/proof-engine/`, not committed)
- `gate1-build.log`, `gate1-build-postfmt.log`, `gate1-test.log`: the test build and the 75 + 56 green run.
- `gate2-clippy.log`: clippy exit 0.
- `gate3-bindings.log`, `EnrollTarget.ts`, `Enrolled.ts`: the export and the emitted files.
- `gate5-realdata.log` / `realdata-result.txt`: the real-embedder run on the copy, with the numbers above.

## Seen, left alone
- `post::transcribe::pcm16k` still decodes a whole track for the post pipeline (it needs all of it); its `ponytail` note stands.
- `voice::decode` (crates/voice) keeps the whole-file demux; `opus_ranges` is sample-identical to it (tested) but lives in `meeting`.
- `Profiles::enroll` still merges on a same-name `EditOp::Enroll` (by design: "keep unchanged").
- `MeetingTranscript` is the wire name of `Transcript` (`ts(rename)`), so W2's stub must import `MeetingTranscript`, as the Contract's TS line already does.
