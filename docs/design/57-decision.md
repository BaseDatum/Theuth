# 57 — Feature: decision — a local typed-probability oracle in front of the reasoning model

Status: **proposed** (2026-09-21). Nothing in this doc is built.

Research it rests on (transient, `/tmp/laya-design/`): `sources.md` (the six links Sky gave, code-verified), `theuth-decisions.md` (decision-point inventory of this checkout), `prior-art.md` (calibration, selective prediction, ort rc.13). Durable facts harvested to knowledge: `facts/laya-typed-decision-classifier`, `decisions/theuth-decision-oracle-onnx-laya`, `facts/theuth-repo-location-and-ort-pin`.

## §0 Decisions in one screen

1. **New workspace crate `decision`** (functional name, 00 §1.1): a local classifier that answers *typed questions* — `Choice` (pick one id of N), `Bool` (does the statement hold), `Score` (ordinal level) — with a probability per option, and a **selective gate** that returns `Accept` or `Escalate(reason)`. It never calls the reasoning model itself; the caller keeps today's path on `Escalate`.
2. **Model = laya** (convaiinnovations, Apache-2.0) via the ONNX fp16 export (`sevenreasons/laya-onnx-fp16`, opset 18, one 807 MiB file) run through the **already-bundled ONNX Runtime** (`ort =2.0.0-rc.13`, `load-dynamic`, initialised once per process by `voice::stt::ort_lib`). laya is a text-only ModernBERT-large encoder (421M) with a decision head — one forward pass, no generation, no KV cache. Rejected: a decoder LLM read through label logprobs (1B fp16 = 2 GiB of weights before activations; needs a generation loop; 52's vision model already covers "ask a bigger model"); candle / MLX / llama.cpp (second runtime, not cross-platform); the hosted Jev/TypeSafe API that cua and browser-use call (network round trip and a third party seeing page/transcript text — the point is local and private).
3. **Two interfaces, one engine.** (a) *Pre-filter*: a plugin asks the oracle before spending a reasoning-model turn and short-circuits on `Accept`. (b) *Compound tool*: a tool the reasoning model calls (`browser_do`, later `decide`) that consults the oracle inside the call and, on `Escalate`, degrades to exactly today's output (the candidate list + probabilities) within the same turn. Rejected: oracle **only** as a tool (spends the big turn we wanted to save); oracle **inside** the agent loop as a generic "model router" (00 §1.4 no god-wiring; the agent does not know what a browser element or a brief is).
4. **The caller owns the candidates; the model picks an id.** Every `Choice` is a bounded table of opaque ids with human descriptions, plus a mandatory `none` negative control. Unknown / stale ids fail closed. Executable arguments (selector, coordinates, brief id) never round-trip through the model. (Pattern lifted from jev-ultrafast `validate_choice` and cua PR #3943's candidate table.)
5. **Gate = per family, calibrated, conservative.** Accept only when calibrated top-1 ≥ τ₁ **and** top-1 − top-2 ≥ τ₂ **and** no hard failure (tokenizer/ONNX error, truncation past the state budget, timeout). laya's own `act_logits` head is logged as a feature and not trusted as the gate until shadow data says so. Thresholds live in config per *decision family* (`meeting.topic`, `browser.target`, …), start conservative, and are fitted from the decision log with a time split against a target selective risk — never one global 0.8. Rejected: fixed universal threshold; entropy as the primary signal (add only if it improves held-out risk/coverage).
6. **Shadow first.** Every family ships in `shadow` mode: the oracle runs, logs, and nothing changes. `active` is flipped per family in config after the log shows acceptable selective risk. Mutating families (anything that clicks or submits) additionally keep the existing action-verification step; the oracle never removes a verification, only a model turn.
7. **Deterministic stays deterministic.** `Risk → PolicyAction`, live/dismissed dedupe, cache alias matching, compaction and harvest cadence, todo promotion are not oracle targets (28, 47, 53 as built).
8. **First target: meeting batches** (53). Today every flushed transcript batch is a full meeting-session model turn deciding "call nothing / `brief_search` / `meeting_topic` / research". The oracle answers `Bool("this batch names a subject worth a brief")` and `Choice` over the top-k cached briefs + `none` in <300 ms locally; the model turn happens only on `Escalate` or when research is needed. Second: **browser target selection** (23) as `browser_do`. Third: **readability routing** before 52's two-call vision fallback. **Computer (27) is out of scope for v0**: laya has no vision; it needs a perception stage (accessibility tree / OCR regions → text candidates) as cua does, which is its own doc.
9. **Memory budget is a measured gate, not an assumption.** fp16 weights alone are 807 MiB; the MLX port measured 944 MiB for one request. The engine is lazy-loaded and idle-unloaded (voice STT pattern), one warm `Session` behind an actor (`Session::run` takes `&mut self`). Build step 2 measures RSS; if > 1 GiB we ship an INT8 dynamic-quantised export (`onnxruntime.quantization.quantize_dynamic`, our own artifact, verified against the fp16 logits) or fall back to the 614 MiB multilingual checkpoint.

## §1 What "typed probability response" actually is (facts relied on)

Verified in `rl_common.py` / `agent.py` (NandhaKishorM/laya, convaiinnovations/laya) and by parsing the ONNX graph:

- Input sequence, contiguous token ids, no chat template:
  `[CLS] <qtype> question: <instructions> [SEP] [MASK] <option 0> [MASK] <option 1> … [SEP] <serialized state> [SEP]`
  Choice option text is `label` or `label: description`; Bool ("noul") options are fixed `false: …` / `true: …`; Score options are `level N: criterion`. Each option ≤ 48 tokens (head budget `head_max_len=192` may shrink further); the state gets the remaining budget and is right-truncated. Root default `max_len=512`. Literal `[MASK]` in user text is replaced by a space.
- Tokenizer: fast BPE `tokenizer.json` (3.58 MB); `[CLS]=50281 [SEP]=50282 [PAD]=50283 [MASK]=50284 [UNK]=50280`. The `tokenizers` crate (0.23.2) is already in our tree via parakeet-rs.
- ONNX inputs: `input_ids i64[B,S]`, `attention_mask i64[B,S]`, `marker_pos i64[B,K]`, `marker_mask bool[B,K]`, `qtype i64[B]` (0 choice, 1 score, 2 noul). Outputs: `logits f16[B,K]` (one score per `[MASK]` marker — **not** vocabulary logits), `act_logits f16[B,2]`. 242 initialisers, no external data, no `past_key_values`.
- Post-processing: mask invalid options to −1e4; temperature = `temperature_by_options[k]` bucket if present else per-qtype `temperature` (from `rl_agent_config.json`); softmax over the first K logits; map back to caller labels in insertion order. `act_probability = softmax(act_logits)[0]`. Confidence: entropy-derived for Choice, `max(p, 1−p)` for Bool.
- Calibration reality: the card itself reports root ECE 0.466 out of domain, 0.081 after per-domain temperature fitting; on the typed-decisions benchmark the un-fine-tuned root scores *below* the majority baseline. **Treat the shipped temperatures as a starting point and expect to fit our own per family.**
- Speed claims (unreproduced): T4 GPU 40 ms/1 question, 159 ms/10; "CPU route mode" 193–464 ms/request. No M-series number exists anywhere; build step 2 produces ours.
- jev-ultrafast: DOM-only (no screenshots), one request = `operation` choice + speculative per-operation `target` choices; median 178 ms per request; executes argmax, **no confidence threshold** — the gap this doc fills. cua PR #3943: perception (`cua-perception` regions) → bounded candidate table → `Choice`; `none` control must abstain; fail closed on unknown ids.

## §2 Crate `decision`

Placement: library beside `search`/`models`, depends on `runtime` (plugin, data dir) and `tools` (for the optional `decide` tool). Consumers: `meeting`, `browser`, later `agent` (readability routing). Dependency line for 00 §10: `runtime ← tools ← … ← decision ← {meeting, browser}`. `decision` never depends on `agent`.

```
crates/decision/src/
  lib.rs        Plugin + DecisionConfig + actor registration (`decision` in the kameo registry)
  question.rs   Question / Candidate / Decision / Disposition / Family types (the public API)
  laya.rs       build_sequence port, tokenizer, ort Session, temperatures → raw probabilities
  gate.rs       selective gate: thresholds per family, reasons, gate version
  engine.rs     actor: lazy load, idle unload, spawn_blocking inference, single warm Session
  log.rs        decision log (append-only JSONL per day under <data_dir>/decisions/) + outcome patching
  tools.rs      `decide` tool (ReadOnly) — phase 3, behind config
```

### Public API (question.rs)

```rust
pub struct Candidate { pub id: String, pub text: String }          // id opaque to the model; text ≤ 48 tokens
pub enum Question {
    Choice { instructions: String, candidates: Vec<Candidate>, state: String },  // `none` appended by the engine
    Bool   { statement: String, state: String },
    Score  { instructions: String, levels: Vec<String>, state: String },
}
pub struct Ask { pub family: Family, pub question: Question, pub budget: Duration }
pub struct Decision {
    pub family: Family,
    pub choice: String,                // candidate id, "none", "true"/"false", or level index
    pub probabilities: Vec<(String, f32)>,   // calibrated, insertion order, sums to 1
    pub confidence: f32,               // entropy-derived (Choice/Score) or max(p,1-p) (Bool)
    pub act_probability: f32,          // laya's act head, logged only
    pub disposition: Disposition,      // Accept | Escalate(Reason) | Shadow(Box<Decision>)
    pub latency: Duration,
    pub gate_version: u32,
    pub log_id: LogId,                 // for `Outcome` patching
}
pub enum Reason { LowTop1, LowMargin, HardError(String), Truncated, Timeout, Disabled, ModelMissing }
pub enum Outcome { Confirmed, Corrected { actual: String }, Unknown }
```

Messages (kameo, owned by this crate): `Ask → Decision`, `Report { log_id, outcome }`, `Stats { family } → FamilyStats` (coverage, escalation reasons, p50/p95 latency, agreement rate in shadow).

Rules: `state` and `instructions` are treated as untrusted content (28) — they are data to the classifier, and the classifier's output is a probability vector, so prompt injection can at worst cause an `Escalate` or a wrong pick that the existing verification catches. The engine strips `[MASK]` and control characters; it never logs secrets (callers pass already-redacted text — the meeting transcript and browser element lines contain none).

### Gate (gate.rs)

```yaml
decision:
  enabled: true
  model_dir: ~/.theuth/models/decision      # model.onnx + tokenizer.json + rl_agent_config.json
  threads: 4                                # intra-op; benchmark, don't default to all cores
  idle_unload: 10m
  families:
    meeting.topic:   { mode: shadow, tau_top: 0.85, tau_margin: 0.40 }
    meeting.brief:   { mode: shadow, tau_top: 0.80, tau_margin: 0.35 }
    browser.target:  { mode: shadow, tau_top: 0.90, tau_margin: 0.50 }
    browser.wait:    { mode: shadow, tau_top: 0.80, tau_margin: 0.30 }
    agent.readable:  { mode: shadow, tau_top: 0.85, tau_margin: 0.40 }
```

`mode: shadow | active | off`. In `shadow` the engine computes the decision, logs it, and returns `Disposition::Shadow(inner)` so the caller proceeds exactly as today but can `Report` the outcome (what the model turn ended up doing). The gate version increments whenever thresholds or the model artifact change so log rows are comparable.

Threshold fitting is a procedure, not code in v0: export the family's log rows with outcomes, split by time (oldest 60 % fit / 20 % calibrate / newest 20 % test), fit one temperature per family, and pick the most permissive (τ₁, τ₂) whose upper-bound selective error on the test split is under the family's target risk (e.g. 2 % for `browser.target`, 10 % for `meeting.topic`); record achieved coverage in the doc's as-built section. A `decision fit` subcommand is a later step if we do this more than twice.

### Engine (engine.rs, laya.rs)

- Startup: `Plugin::start` reads `rt.section("decision")`; if disabled or the model dir lacks `model.onnx` + `tokenizer.json` + `rl_agent_config.json`, registers the actor in `ModelMissing` state (every `Ask` → `Escalate(ModelMissing)`, logged once) and re-checks on the next `Ask` — the files may land while the daemon runs.
- **Distribution = the existing model pipeline (54 §9), not Hugging Face.** `scripts/release-models.sh` (`just release-models`) uploads the `REQUIRED` list from `~/.theuth/models/` to `s3://theuth/releases/models/<model>/<file>` and writes `models.json` (`{files:[{path,size,sha256}]}`); `crates/desktop/src/models.rs` downloads what the manifest lists and the machine lacks, at app start, lazily used by the daemon. Our checkpoint (`decision/model.onnx`, `decision/tokenizer.json`, `decision/rl_agent_config.json` — the fp16 export or our own INT8 re-export, §0.9) is added to `REQUIRED` and published like the speech models; the ONNX Runtime dylib is already shipped in `Contents/Frameworks` by `package-macos.sh`. The generated config comment points at the bucket manifest, not at HF URLs. Open: the manifest has no optional groups today, so every desktop user downloads ~0.8 GiB more on first start — §9.6.
- ONNX Runtime: `voice::stt::ort_lib(dir)` is the process-wide initialiser today — it moves to a tiny shared home so `decision` does not depend on `voice` (see §6 step 1). Enable the workspace `ort/half` feature (rc.13 gates typed `half::f16` tensors behind it; nothing in the tree reads fp16 tensors yet). CPU EP first; CoreML EP is a step-2 experiment behind `decision.coreml: true`, and the engine logs the actual providers/partitions so a silent CPU fallback is visible.
- One `Session` owned by the actor; inference in `spawn_blocking`; requests serialised (rc.13 `run(&mut self)`). Concurrency is by batching rows (`[B,S]`) for callers that ask several questions at once (jev's operation + speculative targets pattern) — the meeting pre-filter asks `Bool` and `Choice` in one batch.
- Budget: `Ask.budget` (default 400 ms) — over budget → `Escalate(Timeout)` while the run finishes in the background and is still logged; a stale answer never acts.
- Lazy load on first `Ask`, unload after `idle_unload` (meeting sessions keep it warm; a quiet desktop releases ~1 GiB).

### Log (log.rs)

`<data_dir>/decisions/YYYY-MM-DD.jsonl`, one row per `Ask`: ts, family, gate_version, model sha, mode, question hash (sha256 of the rendered sequence — not the text), K, raw logits (f32), temperature, calibrated probs, act_p, disposition, reason, latency_ms, then `outcome` patched by `Report` (append a second row keyed by log_id; readers fold). Text is not stored by default (`decision.log_text: false`) — transcript and page text belong to their owners (53, 28). `Stats` folds the current file; older files are for offline fitting.

## §3 First integration: meeting (53)

Today (`meeting/src/lib.rs:1150-1215`, `tools.rs:24-50`): each flushed batch is steered into the meeting session; the reasoning model decides among nothing / `brief_search` / `meeting_topic` / research; cached alias hits (`cache.rs:395-434`) already bypass the model and stay as they are.

With `decision`:

1. Batch flushes (unchanged, `batch.rs`). If no alias hit, the plugin builds one `Ask` batch:
   - `Bool("The speakers mention a specific ticket, system, person, project, number or term the listener may need to look up", state = batch text with speaker tags)` — family `meeting.topic`.
   - `Choice(instructions = "which cached brief is the subject being discussed", candidates = top-8 `brief_search` FTS rows as `{id, title + aliases}`, state = batch text)` — family `meeting.brief`. Skipped when FTS returns nothing.
2. Dispositions:
   - `topic=false` **Accept** → no model turn. The batch is appended to the meeting frame silently so the next escalated turn still has context (today's behaviour keeps the whole transcript anyway).
   - `brief=<id>` **Accept** → publish `meeting_topic` with that cached brief directly (same path as an alias hit, same dedupe against live/dismissed). No model turn.
   - anything **Escalate** → today's path, with one evidence line appended to the batch: `oracle: topic 0.62, brief none 0.55 / SRE-2386 0.31 (escalated: low margin)` — the model sees the probabilities as data (28), which also gives us the labelled outcome for the log.
   - `Shadow` → today's path; after the turn, the plugin reports `Outcome`: `Confirmed` if the model called nothing/`meeting_topic` with the same brief, `Corrected{actual}` otherwise.
3. `[meeting request …]`, `[meeting tapped]`, `[meeting deeper]` never go through the oracle — they are the user asking.

Expected effect: in a typical meeting most batches contain nothing worth a brief; each of those is a full model turn today (seconds and tokens) and becomes a ~200 ms local call. Wrong `false` = a missed brief (recoverable: the user can tap or ask); wrong brief id = an irrelevant cached brief on the island (visible, dismissible). Both bounded — which is why this family goes `active` first.

## §4 Second integration: browser (23)

Today the model lists elements (`browser_elements` / `browser_read`), reads `ref=e3 button "Sign in"` lines, then acts — two turns per action, the second one containing the whole element list. `browser_do(intent, operation?)` (Mutating, like `browser_click`):

1. Snapshot elements (existing `session.rs`), pre-rank to ≤ 10 candidates with the `search` crate's fuzzy match against `intent` (laya's 512-token budget cannot take hundreds of elements), add `none`.
2. `Ask` batch: `Choice(operation ∈ {click, fill, select, press, wait, none})` + speculative `Choice(target)` — one forward pass, jev's shape. Family `browser.target`.
3. **Accept** → execute the existing tool path for that ref (same navigation wait, same post-action snapshot, same risk/policy — `browser_do` declares `Risk::Mutating`, so policy asks exactly as it would for `browser_click`). Reply = today's click reply + `chose e3 (0.94) over e7 (0.04)`.
4. **Escalate** → reply is the candidate list with probabilities and the reason — i.e. what `browser_elements` returns today plus evidence — and the model picks in this same turn with `browser_click`. Failure mode = current behaviour, one extra local call.
5. `browser_wait` condition selection is a second, low-risk family (`browser.wait`) inside the same tool: `Choice` over `{text: "<visible heading>", selector: …}` candidates derived from the intent.

Not changed: refs stale after navigation still fail closed; `site_learn`/routes stay heuristic (`routes/src/classify.rs`) — they are already local and deterministic.

## §5 Third integration: readability routing (52)

Before 52's two-call vision fallback (`agent/src/loop_.rs:4354-4578`), the agent asks `Bool("the page text above is sufficient to complete the current step without seeing the screenshot", state = last snapshot text head + element count)` — family `agent.readable`. `true` Accept → skip the consultation this turn. This is the only agent-side use and it is a pre-filter of an *auxiliary* call, so `agent` depends on `decision` only through the runtime registry lookup, not a crate edge (00 §10 keeps `decision` below `agent` either way).

## §6 Build steps (each ends in a runnable check; one commit each)

1. **Shared ORT init.** Move `ort_lib()` from `crates/voice/src/stt.rs:36-98` to `crates/models/src/onnx.rs` (or a new tiny `onnx` module in `runtime` if `models` is the wrong owner — decide by who else imports `models`); `voice` and `meeting` call it from there. Enable `ort/half` + `half` in the workspace. Check: `cargo check`, voice STT and diarisation unchanged (`just test-voice` or the existing manual check in 45).
2. **`decision` crate, engine only.** `laya.rs` port of `build_sequence` + temperatures; a `decision bench` example that loads `~/.theuth/models/decision`, runs the 8 README examples from NandhaKishorM/laya and prints probabilities, RSS (`ru_maxrss`), cold load and warm p50/p95 for K=2/5/10 at S=256/512. Check: probabilities match the published Python examples to ≤ 0.01 (the ONNX README's own PyTorch↔ONNX tolerance is 0.004 on logits); RSS and p95 recorded in this doc's as-built table. If RSS > 1 GiB: produce the INT8 export and repeat.
2b. **Publish the checkpoint.** Add `decision/model.onnx`, `decision/tokenizer.json`, `decision/rl_agent_config.json` to `REQUIRED` in `scripts/release-models.sh` (comment: `decision/src/laya.rs Engine::load`), run `just release-models` (`THEUTH_S3_DRY_RUN=1` first), verify the anonymous HEADs, then start the desktop app on a Mac without the files and watch `models-progress` fetch them and the daemon leave `ModelMissing` without a restart. If step 2 produced the INT8 artifact, that is the one published; the fp16 file stays local as the reference. Check: `curl -sI https://s3.rwx.dev/theuth/releases/models/decision/model.onnx` → 200 with the manifest's size.
3. **Gate + log + plugin.** `gate.rs`, `log.rs`, config section, actor, `Stats`. Check: unit tests for gate arithmetic (τ₁/τ₂/hard errors), a log round-trip, `ModelMissing` when the dir is empty; `theuth` boots with `decision.enabled: false` and with the model present.
4. **Meeting pre-filter in shadow.** §3 wiring with `Shadow` + `Report`. Check: run one recorded meeting through `meeting_read` replay (53 has the transcript store) and inspect `Stats`: agreement rate, escalation reasons, latency. Fit `meeting.topic` thresholds per §2; flip to `active`; re-run the replay and count saved turns.
5. **`browser_do` in shadow → active** (§4). Check: the 23 smoke pages (login form, search results, paginated table) complete with fewer model turns and no wrong click at the fitted τ; `Escalate` path exercised by an ambiguous intent.
6. **Readability routing** (§5), shadow → active. Check: a text-only model session on a text-heavy page skips the vision consultation; an image-only page still triggers it.
7. **`decide` tool** (Risk ReadOnly; returns the `Decision` as structured output) only if step 4–6 show the model wanting ad-hoc sub-decisions. Prompt block registered only with the tool.

## §7 Frontend / owner surfaces

- Meeting island: an escalated batch shows nothing new; an oracle-accepted brief shows exactly like an alias hit. A small `⚡` marker on oracle-published topics (53's topic row already carries a source enum: add `Oracle`).
- TUI `/decision` (or `Stats` via RPC `decision_stats`): per family — mode, coverage, escalations by reason, p95, shadow agreement. That is how Sky decides to flip `active`.
- Config comments in `theuth/src/main.rs` generated config: model dir, that the desktop app fetches the checkpoint from the release bucket (54 §9), the shadow→active procedure in two lines.

## §8 Security boundary (28)

Inputs to the classifier are untrusted text; outputs are a probability vector over caller-supplied ids — nothing the model emits is executed, so injection in a page or a transcript cannot name a selector or a tool. Risk is declared by the acting tool (`browser_do` = Mutating) and policy is unchanged; the oracle removes model turns, never approvals. The log stores hashes not text by default. The model dir is user-placed and its sha256 is recorded in each log row; a swapped model changes `gate_version` and resets the family to `shadow` (config check at startup).

## §9 Open questions for Sky

1. Doc number: 57 is free in the checkout; another session claimed 58 for the cross-platform doc — confirm 57 is ours.
2. Home for the shared ORT initialiser: `models` (already the model-catalog crate) vs a new `onnx` module in `runtime`.
3. Do we want the INT8 export regardless of the RSS measurement (cross-platform: Windows/Linux CPUs without fp16 fast paths will be slower on fp16 than int8)? My lean: yes, make our own quantised artifact the shipped default and keep fp16 as the reference.
4. Fine-tuning: the card is explicit that laya's headline numbers come from fine-tuning on the target family. Our decision log *is* a training set. Plan for a `typed-decisions`-style fine-tune of the meeting/browser families after a few weeks of shadow data, or stay inference-only?
6. Model download size: `models.json` is one flat list, so adding the oracle makes every first start fetch ~0.8 GiB more (INT8 ≈ 0.4 GiB). Accept, or add an optional `group` field to the manifest so the desktop fetches `decision/*` only when `decision.enabled` (a small change to `release-models.sh` + `desktop/src/models.rs`)? My lean: INT8 + flat list for now, groups when a second optional model appears.
5. `browser_do` vs extending `browser_click` with an `intent` argument — new tool is clearer for the model; extension is less surface. I lean new tool.

## §10 Amendments to 00

- §10 crate list: add `decision` — "local typed-probability oracle (laya via ONNX Runtime) with selective gate; consumers `meeting`, `browser`, `agent` (readability)". Dependency order: `… ← tools ← decision ← {meeting, browser}`; beat: a router inside `agent` (god-wiring) and a model-family crate name (branding).
- §9 data dir: `models/decision/` (model.onnx, tokenizer.json, rl_agent_config.json — published by `release-models.sh`, fetched by the desktop model downloader like the speech models) and `decisions/*.jsonl`.
- §4: note that a plugin may consult another plugin's classifier through the registry (`Ask → Decision`) but a `Tool` must not hide a paid model call; the oracle is local and its cost is latency only.
- §13 (returned subsystems): none — this is new capability.

## As built — step 1 (2026-09-21)

Two commits, `cargo check --workspace` green after each; the brief was `/tmp/laya-design/omp-step1.md` plus Sky's correction that models come from the release bucket, not Hugging Face.

### Part A — shared ONNX Runtime init (`abc13e8`)

- `crates/models/src/onnx.rs`: `pub fn ort_lib(hint_dirs: &[&Path]) -> Result<PathBuf, String>` (same search order — bundle `Contents/Frameworks` / beside the exe → any `libonnxruntime*` in the hint dirs → `$ORT_DYLIB_PATH` → Homebrew — same once-per-process `OnceLock`, same `no libonnxruntime… ` error text so the `contains("libonnxruntime")` matches in `voice::live`/`voice::stt` tests still work) and `pub fn ort_ready() -> bool`. Home = `models` (§9 q2 resolved that way: `voice`, `meeting` and `decision` all already sit above it; `runtime` stays ≤ 400 lines). Windows names (`onnxruntime.dll`) handled while there.
- `voice::stt::ort_lib` deleted; `Engine::load` / `Stream::open` call `models::onnx::ort_lib(&[dir])`. It was `pub(crate)`, so no re-export. `meeting::post::diarize::session` now calls it itself with the model file's directory — the header comment no longer claims `voice::Engine::load` must run first.
- Workspace `ort` features `["load-dynamic", "half"]`, `half = "2"` added; Cargo.lock still has one `ort 2.0.0-rc.13` and `half 2.7.1` (already transitive).

### Part B — crate `decision` (`f0a7339`)

Files: `crates/decision/{Cargo.toml, src/lib.rs, question.rs, gate.rs, laya.rs, engine.rs, log.rs}`; `tools.rs` is step 7. Wired as `decision::DecisionPlugin` after `meeting` in the interactive/headless plugin vector (not the `--acp` vector, like `voice`/`meeting`); generated-config block added after `meeting:`; `scripts/release-models.sh` REQUIRED gains `decision/model.onnx`, `decision/tokenizer.json`, `decision/rl_agent_config.json` (`# decision/src/laya.rs Engine::load`) — **not run**: the files are not local yet (step 2b).

Deviations from §2, and why:

- `Reason` is serde adjacently tagged (`{"reason":"hard_error","detail":"…"}`) — an internally tagged newtype over `String` cannot serialise; found by the log round-trip test.
- `FamilyGate::decide` is the mode-independent threshold verdict; `FamilyGate::finish(Decision)` applies the mode (`Shadow` → `Disposition::Shadow(Box<inner>)` via `Decision::shadowed`, `Off` → `Escalate(Disabled)`). The gate cannot wrap a `Decision` it has not seen, so the two steps are split.
- `gate_version` = first 4 bytes of sha256(tau_top bits, tau_margin bits, model sha); `mode` is deliberately not part of it so shadow→active keeps rows comparable.
- `DecisionConfig.idle_unload` is a humantime string (`10m`) as in the §2 YAML, not `*_secs` like `voice`. Listing any `families:` entry replaces the whole default table (serde map semantics), noted in the config comment.
- The actor is registered even with `enabled: false` (answers `Escalate(Disabled)`), so a consumer's `lookup_when("decision")` never hangs; nothing logs at info level when disabled.
- Engine with the checkpoint present returns `Escalate(HardError("inference not built yet (57 §6 step 2)"))` — truthful until step 2 lands the `Session`; `Loaded` exists with tokenizer/config/model-sha fields and is never constructed yet. Missing files are re-checked on every `Ask`; the `ModelMissing` info line logs once per process.
- Unknown families fail closed (`Escalate(Disabled)`); `Candidate` renders as `id: text` (upstream `label: description`); the `none` control is still the engine's job in step 2.
- `laya::build_sequence` is a line-for-line port of `rl_common.py` (`render_options`, even option shrink when `head_max_len − Σ options < 16`, instructions ≥ 8 tokens, right-truncated state, markers past `max_len` dropped); `Sequence.truncated` = state cut or marker lost. `LayaConfig` deserialises `max_len`, `head_max_len`, `temperature[3]`, `temperature_by_options` from `rl_agent_config.json`; `temperature(qtype, k)` = `temp_bucket` lookup then per-qtype fallback. `softmax_with_temperature` floors `t` at 1e-3 as `agent.py`; `entropy_confidence` = `confidence_from_probs`.
- Tests use a hand-built WordLevel tokenizer (JSON spec via `Tokenizer::from_bytes`); one test additionally loads the real `tokenizer.json` from `~/.theuth/models/decision/` or `/tmp/laya-design/` when present (it was, and confirms the four special ids) and skips otherwise.
- `ort`, `half`, `tools` are declared dependencies (as briefed) but unused until step 2/7.

### Commands and results

- `cargo check --workspace` — green after A and after B (`Finished dev`; the only warning is the pre-existing `block v0.1.6` future-incompat note).
- `cargo test -p voice -p meeting -p models` — 32 + 9 + 13 = 54 passed, 0 failed (models' 13 include the catalog tests; no new tests for `onnx.rs` — its behaviour is the existing voice/meeting load path).
- `cargo test -p decision` — 23 passed, 0 failed (gate 8, laya 9, log 3, engine 2, config 1).
- `cargo clippy -p models -p voice -p meeting --no-deps` and `cargo clippy -p decision --no-deps --tests` — 0 warnings. Without `--no-deps` clippy stops in `crates/tools` (`registry.rs:91` type complexity, too-many-arguments, `if..else` chain) — pre-existing, untouched here.
- `./target/debug/t --help` prints usage; `t --data-dir <tmp> --headless` with `decision: {enabled: false}` logs only the harness's generic `started plugin="decision"` line and nothing from the crate; a fresh data dir's generated `config.yaml` contains the `# decision:` block with the five families.

### Open

- Step 2 (Session, `spawn_blocking`, batching, idle unload, RSS/latency bench) and 2b (publish the checkpoint via `just release-models` once the files are in `~/.theuth/models/decision/`).
- Whether instruction truncation (head budget) should also set `truncated` — today only the state cut and lost markers do, per §0.5.

## As built — step 2 (2026-09-21)

Brief: `/tmp/laya-design/omp-step2.md`; Sky's decision: **fp16 only, no quantisation**. Commits `198eb38` (session + engine), `eb54942` (bench + model-gated tests), this note. Checkpoint under test: `~/.theuth/models/decision/` — `model.onnx` 846,269,825 B (sha256 `1e0031fd826e…`), `tokenizer.json` 3,583,228 B, `rl_agent_config.json` (`max_len` 512, `head_max_len` 192, temperatures `[1.637, 1.251, 1.983]`, six option buckets). ONNX Runtime 1.27.0 dylib from `~/.theuth/models/parakeet-tdt/`, CPU execution provider, Apple M3 Max.

### What landed

- `laya::Loaded::open(dir, threads, coreml)`: `models::onnx::ort_lib(&[dir, dir/../parakeet-tdt])` (the dylib ships beside the TDT checkpoint, 54 §9 — without the sibling hint a daemon whose first `ort` user is `decision` found no runtime), `Session::builder` Level3 + `with_intra_threads`, `tokenizer.json`, `rl_agent_config.json` → `LayaConfig`, streamed sha256 (`io::copy` into `Sha256`), `verify_graph` over `session.inputs()/outputs()` — exactly `input_ids i64[B,S]`, `attention_mask i64[B,S]`, `marker_pos i64[B,K]`, `marker_mask bool[B,K]`, `qtype i64[B]` in, `logits`/`act_logits` Float16 out; names, element types and ranks; extra/missing/renamed/fp32 refused with a message naming the outlet (unit-tested with `Outlet::new`). `coreml: true` logs one warning and runs on the CPU.
- `laya::collate` = `collate_items` (PAD 50283 / attention 0 / marker 0 + mask false); `Loaded::run(&mut self, &[Sequence]) -> Vec<RowOut>`: five tensors via `ort::inputs!`, `try_extract_tensor::<half::f16>()`, `f16_to_f32`, then `postprocess` per row: `temp_bucket` temperature else per-qtype, softmax over the row's own K, `act_p = softmax(act_logits)[0]`, confidence entropy-derived (choice/score) or `max(p, 1−p)` (noul). Raw logits returned for the log.
- `engine.rs`: `Oracle` trait (`Loaded` in production, a `Fake` with a sleeping `run` in the timeout test) behind `Arc<parking_lot::Mutex<Box<dyn Oracle>>>`; `Loader` closure so tests inject. First `Ask` with files present: `Loaded::open` in `spawn_blocking` **within `Ask.budget`** — a load that outruns the budget answers `Escalate(Timeout)` and the `JoinHandle` is polled by the next `Ask` (the 2.2 s cold load never blocks a caller past its budget). Per `Ask`: `with_none` (appends `{none, "none of these"}` once to a `Choice`) → `build` + `run` in one `spawn_blocking` closure → `decide` (labels zipped with probs, argmax, `gate.decide` with `Truncated` as the hard error, `gate.finish`) → log row with `question_hash`, logits, temperature, `load_ms` on the first row after a load → `Decision`. Over budget: `Escalate(Timeout)` now; a spawned task awaits the blocking run and appends its row (real probabilities, reason `timeout`, or the hard error) when it lands. Idle unload: after each run a self-`tell(Unload)` watchdog sleeps `idle_unload`, re-arms if something asked since, else drops the `Arc` (the arena is freed when the last in-flight run finishes). `model_sha` is cached on the engine so no `Ask` path takes the session lock on the actor thread (the first draft did, and a 300 ms run made a 50 ms budget take 312 ms).
- `examples/bench.rs`, `tests/model.rs` as briefed; `tracing-subscriber` and `libc` as dev-deps only.

### Measurements (`cargo run --release -p decision --example bench -- ~/.theuth/models/decision <threads>`)

Cold load (open + verify + sha256): **2.2 s** (2,203 / 2,234 / 2,187 ms at 4 / 8 / 12 threads; sha256 alone ≈ 2.1 s of it in release — in a debug build the same sha2 takes ~28 s, which is why the first test runs looked like a 30 s load). RSS (`ru_maxrss`): **6 MiB before, 1,160 MiB after load, 3,978 MiB after the first inference** and flat from there. **RSS did not stay under 1 GiB: ~1.16 GiB loaded, ~3.9 GiB peak once running.** S is the real sequence length after `build_sequence` (the "512" rows have the state right-truncated to fit `max_len`); K excludes `none` here (bench calls `Loaded` directly).

| K | S | p50 / p95 ms, 4 thr | p50 / p95 ms, 8 thr | p50 / p95 ms, 12 thr |
|---|---|---|---|---|
| 2 | 339 | 763 / 787 | 736 / 908 | 513 / 603 |
| 2 | 512 | 1153 / 1164 | 984 / 1014 | 817 / 1037 |
| 5 | 393 | 865 / 874 | 701 / 726 | 591 / 758 |
| 5 | 512 | 1194 / 1250 | 904 / 1035 | 772 / 820 |
| 10 | 473 | 1146 / 1175 | 1038 / 1171 | 698 / 790 |
| 10 | 512 | 1332 / 1384 | 1046 / 1234 | 760 / 777 |
| README example, B = 4 rows in one pass | ≤ 512 | 934 | 593 | 507 |

So on this machine the fp16 CPU path is **0.5–1.3 s per question, not the <300 ms §0.8 assumed**, and the process holds ~4 GiB while warm. [INFERENCE] The ~3.9 GiB and the latency are consistent with the CPU EP having no fp16 kernels for the encoder's MatMuls and inserting casts (fp16 weights + fp32 copies + fp32 activations); a plain fp32 export would likely use *less* memory and run at the same speed, and INT8 less still — both out of scope by Sky's decision, recorded for the cross-platform doc (58). Threads scale sub-linearly (4 → 12 threads ≈ 1.5× at S ≈ 340); `threads: 4` stays the default per §2's "benchmark, don't default to all cores".

### Agreement with the published example

Only one worked example anywhere gives numbers: the billing e-mail in the GitHub README "Quickstart" and the model card "Single-Model Mode" (identical): department → `billing (confidence: 0.94)`, urgency → `1.84 / 2.0`, churn_risk → `0.892`; refund_requested has no number; the Hindi row uses `laya-multilingual` (not our checkpoint). The ONNX repo README publishes no probabilities, only its own PyTorch↔ONNX deltas (max logits 0.00416, action-prob 0.0).

| question | ours, fitted T | ours, T = 1 | published | within 0.01 |
|---|---|---|---|---|
| department | billing 0.965, technical 0.014, sales 0.010, other 0.011; confidence 0.86 (T 1.76) | billing 0.999 …; confidence 0.99 | billing (confidence 0.94) | argmax yes, number no |
| urgency | 0 0.116, 1 0.327, 2 0.557; score 1.44 (T 1.25) | score 1.52 | score 1.84 | no |
| churn_risk | 0.825 (T 1.98) | 0.956 | 0.892 | argmax yes, number no |
| refund_requested | 0.843 (T 1.98) | 0.965 | — | — |

Every argmax agrees; no probability is within 0.01 at either temperature, and the published numbers sit *between* our fitted-T and T = 1 values on all three — consistent with the README figures coming from a different checkpoint revision or temperature file than the one on the hub today, not with a rendering bug (the sequence is the byte-for-byte port, and the act head reproduces the ONNX README's saturated action-prob exactly). The 0.01 target is therefore **not met against the README**; it can only be met against a Python run of the same artifacts, which nobody has produced — open question below. Our own consistency checks pass: single-row vs batched answers agree to < 0.02, probabilities sum to 1, the two Bool questions read true as the README implies.

### Deviations from §2, and why

- **`act_probability` saturates to exactly 0.0 / 1.0**: the act head's logits are in the thousands (`[4808, −3942]` on the churn question), so `softmax(act_logits)[0]` is 1.0 in f32. Logged as a feature per §0.5; useless as a signal on this checkpoint.
- **Cold load counted against the first `Ask.budget`.** §2 says "lazy load on first `Ask`" without saying who waits; the engine answers `Escalate(Timeout)` and installs the session when the load lands, so a 400 ms caller waits 400 ms, not 2.2 s. The integration test uses a 60 s budget to see the loaded answer in one call.
- **Both pieces of an `Ask` run in one blocking closure** (build + run under the session lock) so the tokenizer never runs on the actor thread.
- **`RowOut.confidence` for Bool** uses `max(p₁, 1−p₁)` as `agent.py`; Choice/Score use `confidence_from_probs` (1 − normalised entropy). `Decision.confidence` is that value.
- No batching across callers yet (§2 "requests serialised … batching rows for callers that ask several questions"): `Ask` is one question; `Loaded::run` already takes `&[Sequence]`, the meeting pre-filter (step 4) decides the message shape.
- Unit-test count for `laya` grew by four pure tests (collate mixed S/K, f16→f32, postprocess bucket/K, graph verification), the engine by three (none appended once + active accept, over-budget timeout with late log, idle unload + reload).

### Commands and results

- Baseline before the step: `cargo check --workspace` green; `cargo test -p decision` 23 passed.
- `cargo check --workspace` — green (`Finished dev`, 47 s incremental).
- `cargo test -p decision` — lib **30 passed**, `tests/model.rs` **2 passed** (with the checkpoint present; `--release` because the debug sha256 of 846 MB takes ~28 s per open), 0 failed; the actor test prints `decision: model loaded in 2.216735959s from … (sha256 1e0031fd826e)` at info under `--nocapture`, then `bool: [("false", 0.226), ("true", 0.774)]`, `choice: [("SRE-2386", 0.901), ("HR-12", 0.020), ("none", 0.079)]`.
- `cargo clippy -p decision --no-deps --all-targets` — 0 warnings (`crates/tools` still fails workspace-wide clippy; pre-existing, untouched).
- Bench: three full runs (4 / 8 / 12 threads), output above and in `/tmp/laya-design/bench-{1,8,12}.txt`.
- Daemon smoke: `t --help` OK; `t --data-dir <tmp> --headless` with `decision: {enabled: true, model_dir: ~/.theuth/models/decision}` logs `started plugin="decision"` and nothing else from the crate (lazy: no load, no `decisions/` dir until the first `Ask`); the boot then died on `plugin rpc failed to start: Address already in use` because the resident daemon on this machine owns the RPC port (also with `rpc.listen` overridden — the smoke did not get past that, so the first-Ask info line was observed through the integration test instead, as the brief allows). No consumer exists yet by design.
- `scripts/release-models.sh` not run; 58 and `cross-platform/` untouched. Sky's own edits to this doc (§0.9 tiers, §11 training capture) were in the working tree during this step and are **not** part of the step-2 commits.

### Open

1. **Memory and latency miss the design's numbers by 4× / 3×** on the fp16 CPU path. Options for Sky: accept for shadow-mode data collection (400 ms default budget → almost every `Ask` times out today; the late rows still land in the log, so fitting data still accumulates), raise the default budget to ~1.5 s, or revisit the artifact (fp32 or INT8 export — 58's question, per the resolved §9.3). CoreML EP is the other lever and is still the step-2 experiment behind `decision.coreml`, not tried.
2. Agreement to 0.01 needs a PyTorch reference run of *these* artifacts (README numbers are not reproducible from the hub checkpoint); one Python session with the fine-tune notebook's environment would settle it.
3. `idle_unload` 10 m releases ~4 GiB on a quiet desktop but re-pays 2.2 s (plus a `Timeout` on the first `Ask`) each time the meeting picks up again.
