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
9. **Three tiers, not two** (Sky, 2026-09-21). `Escalate` does not go straight to the main session turn. Tier 1 = the local oracle (~200 ms). Tier 2 = a **cheap model side call** — the same typed question (instructions, candidate table, state) put to `agent.escalate_model` — a new agent-loop setting next to `title_model`/`vision_model`, unset by default and inheriting `agent.title_model`, configurable in the desktop Settings → Agent loop section (already the harness's "smart enough, cheap, fast" seam), no tools, must answer with a candidate id or `none`; a valid id is acted on exactly like an oracle `Accept`. Tier 3 = the main session turn, reached only when tier 2 answers `none`/invalid, times out, or the decision needs tools or research (a new meeting topic to look up; a page whose candidates are all wrong). Rejected: main model as the only escalation (that is today's cost for every ambiguous case); a second local model (memory).
10. **Memory budget is a measured gate, not an assumption.** fp16 weights alone are 807 MiB; the MLX port measured 944 MiB for one request. The engine is lazy-loaded and idle-unloaded (voice STT pattern), one warm `Session` behind an actor (`Session::run` takes `&mut self`). Build step 2 measures RSS and latency on Sky's Mac and records them here. **fp16 only for now** (Sky, 2026-09-21): quantisation (INT8/FP8) is out of scope; if the fp16 root exceeds the budget the fallback is the 614 MiB multilingual checkpoint, not a re-export.

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
  # tier-2 model is `agent.escalate_model` (agent loop settings), not a decision key: unset → agent.title_model → session model
  families:
    meeting.topic:   { mode: shadow, tau_top: 0.85, tau_margin: 0.40 }
    meeting.brief:   { mode: shadow, tau_top: 0.80, tau_margin: 0.35 }
    browser.target:  { mode: shadow, tau_top: 0.90, tau_margin: 0.50 }
    browser.wait:    { mode: shadow, tau_top: 0.80, tau_margin: 0.30 }
    agent.readable:  { mode: shadow, tau_top: 0.85, tau_margin: 0.40 }
```

`mode: shadow | active | off`. In `shadow` the engine computes the decision, logs it, and returns `Disposition::Shadow(inner)` so the caller proceeds exactly as today but can `Report` the outcome (what the model turn ended up doing). The gate version increments whenever thresholds or the model artifact change so log rows are comparable.

Threshold fitting is a procedure, not code in v0: export the family's log rows with outcomes, split by time (oldest 60 % fit / 20 % calibrate / newest 20 % test), fit one temperature per family, and pick the most permissive (τ₁, τ₂) whose upper-bound selective error on the test split is under the family's target risk (e.g. 2 % for `browser.target`, 10 % for `meeting.topic`); record achieved coverage in the doc's as-built section. A `decision fit` subcommand is a later step if we do this more than twice.

### Tier 2 — cheap-model escalation (escalate.rs)

`decision` cannot depend on `agent` (00 §10), and the model backend lives in `agent`. So `agent` exposes one typed message on the registry — `SideCall { model: ModelRole, system: String, user: String, budget: Duration } → Result<String>` — the same detached no-tools path `maybe_title` uses (`loop_.rs:3035-3075`), with `ModelRole::{Escalate, Title, Vision, Session, Named(String)}`. `decision::escalate` owns the rest: render the question as a compact prompt (candidate table with ids, state, "answer with exactly one id from the table or `none`, then one line of reason"), call `SideCall` with `ModelRole::Escalate` — resolved inside `agent` from `agent.escalate_model` (unset → `agent.title_model` → session model), so `decision` never sees model ids, parse the first token as an id, **validate it against the table** (anything else = tier 3), and return a `Decision` whose `disposition` is `Accept` with `tier: 2` and `probabilities` = one-hot on the chosen id (tier 2 has no calibrated probabilities; the log records the tier so the two are never mixed in threshold fitting). The tier-2 answer is also the training label when the family is opted in (§11): the prompt asks for the one-line reason, and the example is written from that same response — no second call. Budget default 8 s; timeout → tier 3.

`Decision` gains `tier: u8` (1 oracle, 2 cheap model, 3 main model — the last set by the consumer's `Report`). `Stats` reports coverage per tier so the win is visible as "batches that never reached the main model".

### Engine (engine.rs, laya.rs)

- Startup: `Plugin::start` reads `rt.section("decision")`; if disabled or the model dir lacks `model.onnx` + `tokenizer.json` + `rl_agent_config.json`, registers the actor in `ModelMissing` state (every `Ask` → `Escalate(ModelMissing)`, logged once) and re-checks on the next `Ask` — the files may land while the daemon runs.
- **Distribution = the existing model pipeline (54 §9), not Hugging Face.** `scripts/release-models.sh` (`just release-models`) uploads the `REQUIRED` list from `~/.theuth/models/` to `s3://theuth/releases/models/<model>/<file>` and writes `models.json` (`{files:[{path,size,sha256}]}`); `crates/desktop/src/models.rs` downloads what the manifest lists and the machine lacks, at app start, lazily used by the daemon. Our checkpoint (`decision/model.onnx`, `decision/tokenizer.json`, `decision/rl_agent_config.json` — the fp16 export, §0.9) is added to `REQUIRED` and published like the speech models; **The ONNX Runtime itself is an artifact of the same pipeline** (Sky, 2026-09-21; step 2c): canonical on-disk location `<data_dir>/models/onnxruntime/libonnxruntime.{dylib,so,dll}` + `VERSION` — not beside parakeet — published per platform to `models/onnxruntime/<target-triple>/` from one pinned `scripts/ort-version.env` that `package-macos.sh` also sources for the copy it codesigns into `Contents/Frameworks`. Manifest entries gain optional `platform` and `key` so the desktop downloader fetches only its own platform's runtime. `models::onnx::ort_lib(data_dir)` searches: app bundle (the codesigned copy a hardened-runtime app must use — hence first) → `models/onnxruntime/` → `$ORT_DYLIB_PATH` → Homebrew; no hint dirs. The generated config comment points at the bucket manifest, not at HF URLs. Open: the manifest has no optional groups today, so every desktop user downloads ~0.8 GiB more on first start — §9.6.
- ONNX Runtime: `voice::stt::ort_lib(dir)` is the process-wide initialiser today — it moves to a tiny shared home so `decision` does not depend on `voice` (see §6 step 1). Enable the workspace `ort/half` feature (rc.13 gates typed `half::f16` tensors behind it; nothing in the tree reads fp16 tensors yet). CPU EP first; CoreML EP is a step-2 experiment behind `decision.coreml: true`, and the engine logs the actual providers/partitions so a silent CPU fallback is visible.
- One `Session` owned by the actor; inference in `spawn_blocking`; requests serialised (rc.13 `run(&mut self)`). Concurrency is by batching rows (`[B,S]`) for callers that ask several questions at once (jev's operation + speculative targets pattern) — the meeting pre-filter asks `Bool` and `Choice` in one batch.
- Budget: `Ask.budget` (default 400 ms) — over budget → `Escalate(Timeout)` while the run finishes in the background and is still logged; a stale answer never acts.
- Lazy load on first `Ask`, unload after `idle_unload` (meeting sessions keep it warm; a quiet desktop releases ~1 GiB).

### Log (log.rs)

`<data_dir>/decisions/YYYY-MM-DD.jsonl`, one row per `Ask`: ts, family, gate_version, model sha, mode, question hash (sha256 of the rendered sequence — not the text), K, raw logits (f32), temperature, calibrated probs, act_p, disposition, reason, latency_ms, then `outcome` patched by `Report` (append a second row keyed by log_id; readers fold). Text is never stored in this log — transcript and page text belong to their owners (53, 28); the opt-in training capture in §11 is a separate store with its own consent switch. `Stats` folds the current file; older files are for offline fitting.

## §3 First integration: meeting (53)

Today (`meeting/src/lib.rs:1150-1215`, `tools.rs:24-50`): each flushed batch is steered into the meeting session; the reasoning model decides among nothing / `brief_search` / `meeting_topic` / research; cached alias hits (`cache.rs:395-434`) already bypass the model and stay as they are.

With `decision`:

1. Batch flushes (unchanged, `batch.rs`). If no alias hit, the plugin builds one `Ask` batch:
   - `Bool("The speakers mention a specific ticket, system, person, project, number or term the listener may need to look up", state = batch text with speaker tags)` — family `meeting.topic`.
   - `Choice(instructions = "which cached brief is the subject being discussed", candidates = top-8 `brief_search` FTS rows as `{id, title + aliases}`, state = batch text)` — family `meeting.brief`. Skipped when FTS returns nothing.
2. Dispositions:
   - `topic=false` **Accept** → no model turn. The batch is appended to the meeting frame silently so the next escalated turn still has context (today's behaviour keeps the whole transcript anyway).
   - `brief=<id>` **Accept** → publish `meeting_topic` with that cached brief directly (same path as an alias hit, same dedupe against live/dismissed). No model turn.
   - anything **Escalate** → tier 2 first: the cheap model gets the same two questions; a valid `brief=<id>` publishes that cached brief, `topic=false` skips the turn, both logged with `tier: 2`. Only `none`/invalid/timeout — or `topic=true` with no matching cached brief, which needs research — reaches tier 3, today's path, with one evidence line appended to the batch: `oracle: topic 0.62, brief none 0.55 / SRE-2386 0.31 (escalated: low margin)` — the model sees the probabilities as data (28), which also gives us the labelled outcome for the log.
   - `Shadow` → today's path; after the turn, the plugin reports `Outcome`: `Confirmed` if the model called nothing/`meeting_topic` with the same brief, `Corrected{actual}` otherwise.
3. `[meeting request …]`, `[meeting tapped]`, `[meeting deeper]` never go through the oracle — they are the user asking.

Expected effect: in a typical meeting most batches contain nothing worth a brief; each of those is a full model turn today (seconds and tokens) and becomes a ~200 ms local call. Wrong `false` = a missed brief (recoverable: the user can tap or ask); wrong brief id = an irrelevant cached brief on the island (visible, dismissible). Both bounded — which is why this family goes `active` first.

## §4 Second integration: browser (23)

Today the model lists elements (`browser_elements` / `browser_read`), reads `ref=e3 button "Sign in"` lines, then acts — two turns per action, the second one containing the whole element list. `browser_do(intent, operation?)` (Mutating, like `browser_click`):

1. Snapshot elements (existing `session.rs`), pre-rank to ≤ 10 candidates with the `search` crate's fuzzy match against `intent` (laya's 512-token budget cannot take hundreds of elements), add `none`.
2. `Ask` batch: `Choice(operation ∈ {click, fill, select, press, wait, none})` + speculative `Choice(target)` — one forward pass, jev's shape. Family `browser.target`.
3. **Accept** → execute the existing tool path for that ref (same navigation wait, same post-action snapshot, same risk/policy — `browser_do` declares `Risk::Mutating`, so policy asks exactly as it would for `browser_click`). Reply = today's click reply + `chose e3 (0.94) over e7 (0.04)`.
4. **Escalate** → tier 2 inside the same tool call: the cheap model picks from the same ≤ 10 candidates; a valid ref executes as in step 3 with `tier: 2` in the reply. Only `none`/invalid/timeout falls through: the reply is the candidate list with probabilities and the reason — i.e. what `browser_elements` returns today plus evidence — and the model picks in this same turn with `browser_click`. Failure mode = current behaviour, one extra local call.
5. `browser_wait` condition selection is a second, low-risk family (`browser.wait`) inside the same tool: `Choice` over `{text: "<visible heading>", selector: …}` candidates derived from the intent.

Not changed: refs stale after navigation still fail closed; `site_learn`/routes stay heuristic (`routes/src/classify.rs`) — they are already local and deterministic.

## §5 Third integration: readability routing (52)

Before 52's two-call vision fallback (`agent/src/loop_.rs:4354-4578`), the agent asks `Bool("the page text above is sufficient to complete the current step without seeing the screenshot", state = last snapshot text head + element count)` — family `agent.readable`. `true` Accept → skip the consultation this turn. This is the only agent-side use and it is a pre-filter of an *auxiliary* call, so `agent` depends on `decision` only through the runtime registry lookup, not a crate edge (00 §10 keeps `decision` below `agent` either way).

## §6 Build steps (each ends in a runnable check; one commit each)

1. **Shared ORT init.** Move `ort_lib()` from `crates/voice/src/stt.rs:36-98` to `crates/models/src/onnx.rs` (or a new tiny `onnx` module in `runtime` if `models` is the wrong owner — decide by who else imports `models`); `voice` and `meeting` call it from there. Enable `ort/half` + `half` in the workspace. Check: `cargo check`, voice STT and diarisation unchanged (`just test-voice` or the existing manual check in 45).
2. **`decision` crate, engine only.** `laya.rs` port of `build_sequence` + temperatures; a `decision bench` example that loads `~/.theuth/models/decision`, runs the 8 README examples from NandhaKishorM/laya and prints probabilities, RSS (`ru_maxrss`), cold load and warm p50/p95 for K=2/5/10 at S=256/512. Check: probabilities match the published Python examples to ≤ 0.01 (the ONNX README's own PyTorch↔ONNX tolerance is 0.004 on logits); RSS and p95 recorded in this doc's as-built table. If RSS > 1 GiB: note it and evaluate the multilingual checkpoint (quantisation is out of scope).
2b. **Publish the checkpoint.** Add `decision/model.onnx`, `decision/tokenizer.json`, `decision/rl_agent_config.json` to `REQUIRED` in `scripts/release-models.sh` (comment: `decision/src/laya.rs Engine::load`), run `just release-models` (`THEUTH_S3_DRY_RUN=1` first), verify the anonymous HEADs, then start the desktop app on a Mac without the files and watch `models-progress` fetch them and the daemon leave `ModelMissing` without a restart. The fp16 artifact is what is published. Check: `curl -sI https://s3.rwx.dev/theuth/releases/models/decision/model.onnx` → 200 with the manifest's size.
2c. **Runtime as an artifact.** `models/onnxruntime/` canonical dir, `ort_lib(data_dir)` order above, `scripts/ort-version.env`, `release-models.sh` publishes the dylib per platform with `platform`/`key` manifest fields, desktop downloader filters by platform and `chmod 755`s. Check: `cargo test -p models -p voice -p meeting -p decision` with the dylib only in `models/onnxruntime/`; `THEUTH_S3_DRY_RUN=1 just release-models` lists the runtime entries; `bash -n` on both scripts.
3. **Gate + log + plugin.** `gate.rs`, `log.rs`, config section, actor, `Stats`. Check: unit tests for gate arithmetic (τ₁/τ₂/hard errors), a log round-trip, `ModelMissing` when the dir is empty; `theuth` boots with `decision.enabled: false` and with the model present.
3b. **`SideCall` in `agent` + `decision::escalate`.** Agent gains `agent.escalate_model` (config + generated-config comment + desktop Settings → Agent loop picker, unset → title model) and registers `SideCall` on the bus over the existing detached no-tools path; `decision::escalate` renders/parses/validates and logs `tier: 2`. Check: unit tests for parsing (`e3`, `none`, prose, an id not in the table → tier 3), an integration test with a stub `SideCall` actor, and one real call against `agent.title_model` picking from three obvious candidates.
4. **Meeting pre-filter in shadow.** §3 wiring with `Shadow` + `Report`. Check: run one recorded meeting through `meeting_read` replay (53 has the transcript store) and inspect `Stats`: agreement rate, escalation reasons, latency. Fit `meeting.topic` thresholds per §2; flip to `active`; re-run the replay and count saved turns.
5. **`browser_do` in shadow → active** (§4). Check: the 23 smoke pages (login form, search results, paginated table) complete with fewer model turns and no wrong click at the fitted τ; `Escalate` path exercised by an ambiguous intent.
6. **Readability routing** (§5), shadow → active. Check: a text-only model session on a text-heavy page skips the vision consultation; an image-only page still triggers it.
7. **`decide` tool** (Risk ReadOnly; returns the `Decision` as structured output) only if step 4–6 show the model wanting ad-hoc sub-decisions. Prompt block registered only with the tool.

## §7 Frontend / owner surfaces

- Meeting island: an escalated batch shows nothing new; an oracle-accepted brief shows exactly like an alias hit. A small `⚡` marker on oracle-published topics (53's topic row already carries a source enum: add `Oracle`).
- Desktop Settings → Agent loop: **Escalation model** picker (`agent.escalate_model`), same control as the title/vision model pickers, placeholder "inherits title model"; unset by default.
- Desktop Settings → "Decision training data": per-family opt-in switches (§11.1.2), off by default.
- TUI `/decision` (or `Stats` via RPC `decision_stats`): per family — mode, coverage, escalations by reason, p95, shadow agreement. That is how Sky decides to flip `active`.
- Config comments in `theuth/src/main.rs` generated config: model dir, that the desktop app fetches the checkpoint from the release bucket (54 §9), the shadow→active procedure in two lines.

## §8 Security boundary (28)

Inputs to the classifier are untrusted text; outputs are a probability vector over caller-supplied ids — nothing the model emits is executed, so injection in a page or a transcript cannot name a selector or a tool. Risk is declared by the acting tool (`browser_do` = Mutating) and policy is unchanged; the oracle removes model turns, never approvals. The log stores hashes not text by default. The model dir is user-placed and its sha256 is recorded in each log row; a swapped model changes `gate_version` and resets the family to `shadow` (config check at startup).

## §9 Open questions for Sky

1. Doc number: 57 is free in the checkout; another session claimed 58 for the cross-platform doc — confirm 57 is ours.
2. Home for the shared ORT initialiser: `models` (already the model-catalog crate) vs a new `onnx` module in `runtime`.
3. ~~INT8 export?~~ Resolved 2026-09-21: fp16 only; quantisation out of scope for now (revisit with the cross-platform work in 58 if Windows/Linux CPU fp16 paths prove slow).
4. ~~Fine-tuning?~~ Resolved 2026-09-21: yes — opt-in per-family training capture, composed by the reasoning model in the background, shipped to S3 via relay (§11).
7. ~~Tier 2 model?~~ Resolved 2026-09-21: `agent.escalate_model`, an agent-loop setting exposed in desktop Settings, unset by default, inherits `agent.title_model`.
6. Model download size: `models.json` is one flat list, so adding the oracle makes every first start fetch ~0.8 GiB more. Accept, or add an optional `group` field to the manifest so the desktop fetches `decision/*` only when `decision.enabled` (a small change to `release-models.sh` + `desktop/src/models.rs`)? My lean: flat list for now, groups when a second optional model appears.
5. `browser_do` vs extending `browser_click` with an `intent` argument — new tool is clearer for the model; extension is less surface. I lean new tool.

## §10 Amendments to 00

- §10 crate list: add `decision` — "local typed-probability oracle (laya via ONNX Runtime) with selective gate; consumers `meeting`, `browser`, `agent` (readability)". Dependency order: `… ← tools ← decision ← {meeting, browser}`; beat: a router inside `agent` (god-wiring) and a model-family crate name (branding).
- §9 data dir: `models/onnxruntime/` (the runtime dylib + `VERSION`, published per platform, fetched by the downloader; the bundle copy wins in packaged builds) and `models/decision/` (model.onnx, tokenizer.json, rl_agent_config.json — published by `release-models.sh`, fetched by the desktop model downloader like the speech models) and `decisions/*.jsonl`.
- §5/§9: `agent.escalate_model` joins `title_model`/`vision_model` as the third named model role (unset → title model). `agent` exposes `SideCall { model: ModelRole, system, user, budget } → String` on the registry — the detached no-tools side call generalised from `maybe_title`; consumers are `decision::escalate` and the §11 composer. Beat: giving `decision` its own backend (a second copy of provider auth and cost accounting).
- §4: note that a plugin may consult another plugin's classifier through the registry (`Ask → Decision`) but a `Tool` must not hide a paid model call; the oracle is local and its cost is latency only.
- §13 (returned subsystems): none — this is new capability.

## §11 Training-data capture (opt in, per family, off by default)

Sky's spec (2026-09-21): the escalations and shadow decisions the reasoning model makes are the labelled dataset for each decision family, and we collect them — **only with explicit per-family opt-in**, composed by the reasoning model **in a background call that never blocks the conversation**, shipped to S3 in one standard JSON format under a subdirectory per family, through a new **rate-limited relay endpoint** that holds the only write credential.

### 11.1 Decisions

1. **Capture is separate from the decision log.** §2's JSONL log stays hashed (probabilities, gate, latency, outcome) and always on — it is what threshold fitting needs. Training capture stores the *inputs* and is off unless the family is opted in. Two files, two purposes, one `LogId` linking them.
2. **Opt-in lives in config, edited from the desktop app.** `decision.families.<name>.train: bool` (default `false`, for every family, including future ones — an unknown family cannot be opted in). The desktop Settings gets a **"Decision training data"** section: one switch per family with a one-line description of exactly what a captured example contains (e.g. `meeting.topic`: *"the last transcript batch — other people's speech — and whether a brief was warranted"*), a link to the local copy, and a "delete local + stop uploading" action. Written through the same config path the desktop already uses for other settings; the harness re-reads `decision` families live (no restart).
3. **When a capture happens.** After a decision the reasoning model made — `Escalate` (the model picked because the oracle abstained) or `Shadow` (the model picked, the oracle only watched). `Accept` is never captured: the label would be the oracle's own answer. Trigger = the consumer's `Report{log_id, outcome}`; the engine checks `train` for the family and, if on, enqueues a `Compose` job.
4. **Whoever decided composes the example, never blocking the session.** When **tier 2** decided, the example is written from that same side-call response (id + one-line reason) — no extra call. When **tier 3** (the main session turn) decided, `Compose` is a detached auxiliary call via the same `SideCall` message (no tools, own system prompt, 45 s budget, own cost line) with `ModelRole::Escalate` — the same model that would have decided at tier 2; no separate training-model setting. It never appends to the session frame and never steers it. Input: the rendered oracle question (instructions, candidate table, state exactly as truncated to laya's 512-token budget), the oracle's probabilities and abstain reason, and what the model actually did (`outcome.actual`). Output: one JSON example — the label as a candidate id, optional corrected candidate descriptions (a candidate whose text was misleading), and a one-sentence rationale. The composer must return a candidate id from the table or `none`; anything else is dropped and counted, never coerced (28).
5. **One format, `theuth.decision.v1`**, laya `typed-decisions`-shaped so the fine-tune notebook consumes it without conversion:

   ```json
   { "schema": "theuth.decision.v1", "id": "<ulid>", "ts": "<rfc3339>",
     "family": "meeting.topic", "harness": "1.8.4", "model_sha": "<onnx sha256>", "gate_version": 123,
     "question": { "type": "choice|bool|score", "instructions": "…",
                   "candidates": [{ "id": "…", "text": "…" }], "state": "…" },
     "oracle": { "probabilities": { "<id>": 0.31 }, "act_probability": 0.4,
                 "disposition": "escalate|shadow", "reason": "low_margin" },
     "label": { "choice": "<id>", "rationale": "…", "corrections": { "<id>": "<better text>" } },
     "composer": { "model": "provider/model", "latency_ms": 1800 } }
   ```
   No session id, no user id, no timestamps finer than the example's own. Secrets are scrubbed with the `secrets` crate's placeholder scrubber before the example is written anywhere.
6. **Local copy first, then upload.** `<data_dir>/decisions/training/<family>/<ulid>.json`; an uploader drains the directory with retry/backoff and deletes on 2xx (`decision.train_keep_local: true` keeps them). Flipping a family off drops its queue and stops uploads immediately; the Settings "delete" action removes the local files.
7. **Relay endpoint, relay holds the key.** `POST /train/{family}` on `theuth-relay` (31 §5): body = one example, JSON, ≤ 64 KiB; family must be in relay's allowlist; schema field must be `theuth.decision.v1`; authenticated with the harness's link identity (the same credential the `/h/{id}` socket presents), rate-limited with the existing per-IP token bucket **plus** a per-harness daily cap (`--train-daily-cap`, default 500). Relay PUTs the body unchanged to `s3://theuth/training/<family>/<YYYY>/<MM>/<ulid>.json` (same rustfs endpoint as releases, separate prefix) with a **write-only** credential scoped to `training/*`, given to relay as env (`THEUTH_TRAIN_S3_KEY_ID` / `_SECRET`) — the harness never holds an S3 key. Relay stays stateless: no read-back, no listing; `/metrics` gains `train_accepted`, `train_rejected`, `train_rate_limited`. Rejected alternatives: harness PUTs to S3 directly (a bucket credential on every user's machine); a frame type over the existing WebSocket (relay would have to buffer and reassemble; a plain HTTP PUT is the right shape for a blob).
8. **Fine-tuning is offline and later.** `s3://theuth/training/<family>/` → the laya fine-tune notebook → a new `decision/model.onnx` published through `release-models.sh` with a new `model_sha` → every family resets to `shadow` (57 §8). Not part of these build steps.

### 11.2 Build steps (continue §6 numbering)

8. **Capture + local store.** `decision.families.<name>.train`, `Compose` job type and queue, `training/<family>/` writer, example schema type + serde tests, secret scrub. Check: `Report` on an escalated row with `train: true` writes one valid example locally; with `train: false` writes nothing.
9. **Composer in `agent`.** Detached aux call with the system prompt above; `decision.train_model`; cost line; drop-and-count on an invalid label. Check: a meeting replay with `meeting.topic.train: true` produces examples whose `label.choice` matches what the session actually did; the session's own turn timing is unchanged (measure).
10. **Desktop Settings section.** Switch per family, description text, local-copy link, delete action; harness live-reloads the family table. Check: toggling in the UI changes `config.yaml` and the next `Report` respects it without restart.
11. **Relay endpoint + uploader.** `POST /train/{family}`, allowlist, size/schema checks, link-identity auth, token bucket + daily cap, S3 PUT with the scoped key, metrics; harness uploader with backoff. Check: `curl` without identity → 401; over cap → 429 and `train_rate_limited` increments; a valid example appears at the expected key (`aws s3 ls` with the admin key); uploader deletes the local file on 2xx and keeps it on 5xx.

### 11.3 Amendments to 00 (add to §10)

- `relay` gains an optional S3 write dependency (`training/*` prefix only) and a `/train/{family}` route; §9 data dir gains `decisions/training/<family>/`; config `decision.families.<name>.train`, `decision.train_model`, `decision.train_keep_local`.

## §12 Model-tree survey (2026-09-21, after step 2's numbers) — what the fast deployments actually do

Step 2 measured 0.5–1.3 s per question and ~3.9 GiB warm with the fp16 ONNX on the CPU EP, against the <300 ms / 1 GiB §0 assumed. Sky's instruction: crawl the fine-tunes and quantisations under `convaiinnovations/laya` and find the correct pre-existing setup. All 17 derivatives (HF model tree: 7 fine-tunes, 10 quantisations, 0 adapters) were read — README + file lists. Facts below are from those READMEs; "claimed" means the publisher's number.

### 12.1 Why our numbers were bad — and it is not the model

- **fp16 on the CPU EP is the wrong artifact.** The most-liked ONNX export (`Mattepiu/laya-onnx`) ships fp32 (`laya.onnx`, 1.69 GB) and INT8 (`laya_int8.onnx`, 581 MB) and files its fp16 under **`fp16_onlygpu_unverified/`**. `tozp/laya-onnx` ships fp32 + INT8 (`model_int8.onnx`, 424 MB), no fp16. Nobody runs fp16 on a CPU; the CPU EP has no fp16 kernels for these MatMuls and casts everything (omp's [INFERENCE] in the step-2 note is what the community already knew). Our 3.9 GiB is fp16 weights + fp32 copies + fp32 activations.
- **Every sub-30 ms number is an accelerator**, never a CPU: T4 GPU 32.8–39.5 ms (official card), RTX 4070 Ti 17–23 ms (`cklxx/laya-browser`), RTX 5090 10.7 ms (`Luni/laya-grounded`), Apple ANE/GPU 3.6–27 ms (below), Axera NPU 27.7 ms (multilingual, S256 K4). The official **CPU** number is **193–464 ms** (PyTorch fp32, `Router(preload=True)`); LiteRT measured fp32 CPU 4 threads **222 ms at S=256** (multilingual) on a phone.
- **Fast deployments use fixed shapes.** CoreML `.mlmodelc` buckets L128/L256/L512/L1024 × options32 (`FluidInference/laya-coreml`), ANE L96 (`aac6fef/laya-coreml`), LiteRT s256/s512, Axera B1 S256 K4. Accelerators want static shapes; pad to a bucket, do not feed dynamic S/K.
- **The fast ones mostly use the multilingual checkpoint (mmBERT-base, 322M, 614 MiB fp16, 1024 context)**, not the 421M English root: FluidInference, aac6fef ANE, Axera's fastest row, and the best browser fine-tune (`cklxx` v10s/v11s). Smaller, faster, longer context; the card says it also wins on non-English and is close on English (XNLI-en 0.843 vs 0.860).

### 12.2 macOS: Core ML is the pre-existing answer (Apple silicon, primary platform)

| artifact | checkpoint | shapes | measured (warm, one question) | notes |
|---|---|---|---|---|
| `FluidInference/laya-coreml` | multilingual fp16 | L128/256/512/1024 × 32 options, batch 1 | **M5 Pro: L128 3.6 ms (CPU+ANE), L256 5.2 ms, L512 9.0 ms, L1024 17.9 ms (all units)**; 16/16 argmax parity vs PyTorch fp32, max prob error 0.002 | Swift package + demo; `.mlmodelc` compiled models, tokenizer.json (256k vocab) |
| `aac6fef/laya-coreml` | multilingual fp16 / W8 | ANE L96, 512 total tokens, 32 option slots | **M3 Max: 4.98 / 5.31 ms p50 / p95** (fp16), 4.88 / 5.23 (W8) | `.mlpackage`; docs at github.com/mizorewww/laya-coreml (same author as laya-mlx) |
| `danielamitay/laya-en-fp32-swev` | English fp32 | L4096 K16 mlpackage | no numbers | sliding-window variant |
| `aac6fef/laya-mlx` | root fp16 | dynamic | 943.6 MiB allocated (from §1) | MLX, Python |

Two ways to get this from Rust, same `Oracle` trait, decided by a spike (step 2d):
1. **ONNX Runtime CoreML EP** through the `ort` we already ship — Microsoft's osx-arm64 dylib includes the CoreML EP; register it with `MLProgram` + fp16 compute, pad every request to a bucket (S ∈ {128, 256, 512}, K = 32) so the graph is static, and **log the partition count** — one partition means the whole encoder runs on ANE/GPU; many means unsupported ops fell back to CPU and the win is gone. Zero new dependencies; cross-platform code path (the EP is simply absent elsewhere).
2. **Direct Core ML** via `objc2-core-ml` (macOS-only backend module): load FluidInference's `.mlmodelc` buckets as published — the proven artifact, no ORT conversion risk. "If we need our own Rust kernel, so be it" — this is that path, and it is glue, not a kernel.

### 12.3 Linux / Windows (design 58): fp32 or INT8 ONNX on the CPU EP

fp32 ONNX (Mattepiu `laya.onnx` 1.69 GB, tozp `model.onnx` 1.69 GB; opset 14/18) at the official 200–450 ms; INT8 (tozp 424 MB, Mattepiu 581 MB, `nvkudva/laya-web-q8` split encoder/head 468+53 MB, `p-yan/laya-quanto` q8 481 MB) is the CPU speed path — Sky has ruled quantisation out for now, so the CPU fallback is **fp32**, and a GPU EP (CUDA/DirectML) is 58's problem. GGUF exports (`mys`, `fr0stbit3`: F16/Q8_0/Q6_K/Q4_K_M, with a separate `laya-head.safetensors`) exist but need llama.cpp — rejected in §0.2.

### 12.4 Browser: the base model cannot do it; `cklxx/laya-browser` can

`cklxx/laya-browser` (Apache-2.0) is laya fine-tuned as the decision head for **jev-ultrafast** — the exact `operation + target` request shape §4 planned:

- Base laya at "which element for this goal" among ~45 candidates: **top-1 0.10** (chance). v10 (ModernBERT-large 421M): 0.66; **v10s (mmBERT-base 322M): 0.63 top-1, 62 % of 16 real browser tasks 3/3, 17–23 ms per 3-question step on an RTX 4070 Ti (3 ms single question)**; operation accuracy 0.88–0.89.
- **Format v3** (recorded in each checkpoint's `rl_agent_config.json` as `laya_fmt`, `head_max_len_train` 768): the elements live **only in the option list** (full label + role + current value), the state keeps title / URL / action history / 1.2–1.5 k chars of page text. This replaces §4's "pre-rank to ≤ 10 candidates" — v10s handles 30–65 candidates per step. Our `build_sequence` must honour `laya_fmt`/`head_max_len_train` from the config, not hard-code root defaults.
- Their negative result matters for §0.9: **confidence-gated escalation to Qwen3-8B made the suite worse (58 % → 42 %)**, and a 27B teacher only matched the 322M student at 200× the latency. For `browser.target`, tier 2 must be a strong model or nothing — the title model is not automatically "smart enough" for element choice; `Stats` per tier will show it, and the family gate for tier 2 should be able to say `off`.
- Training data recipe (reverse-generated goals on crawled pages, real DONE landing pages, step-2 negatives, Mind2Web, DAgger corrections) and pitfalls are documented in `code/finetune/README.md` — the template for §11's fine-tune loop. `ShaunSpark/laya-mind2web-browser-agent` (Mind2Web only, 671 examples, 74.3 % val) is the smaller precedent. `Luni/laya-grounded` fixes verbatim-fact grounding and negation consistency but is **CC-BY-NC-4.0 — unusable for us**.
- No ONNX/CoreML export of v10s exists; we export it ourselves (laya's export scripts, as `sevenreasons` did for root) and publish it as `decision/browser/` — **one checkpoint directory per family group**, which §2 must support (`families.<name>.model: browser`).

### 12.5 What changes in this design

- §0.2/§0.10: the shipped artifact is **not** the fp16 root ONNX. macOS: multilingual fp16 via Core ML (ORT CoreML EP or direct); other CPUs: fp32 ONNX. Memory: 614 MiB weights (multilingual) — the 1 GiB budget is back in reach on the Core ML path; measure.
- §0.8 latency assumption (<300 ms) stands **only** on an accelerator; the CPU fallback is 200–450 ms, still 5–10× under a tier-2 call.
- §2: per-family checkpoint dirs; `build_sequence` driven by the checkpoint's `rl_agent_config.json` (`laya_fmt`, `head_max_len_train`, `max_len`), fixed-shape bucketing in `collate`.
- §4: adopt format v3 and `cklxx` v10s for `browser.target`; drop the ≤ 10 pre-rank; tier 2 for this family may be `off`.
- §9.3 (quantisation) stays resolved as "not now", but the record shows INT8 is the accepted CPU path in the tree.
- §6 gains **step 2d** (Core ML spike, below) before 2b publishes anything.

### 12.6 Decisions (Sky, 2026-09-21 evening)

- **English root checkpoint is the base for every family in v1** — not multilingual for now; the browser fine-tune export comes later. v1's priority is **fast meeting briefs**; browser/computer speed can follow.
- **Path A — ONNX Runtime CoreML EP — is the chosen macOS accelerator path; cross-platform is the priority.** One runtime (`ort`), one code path, the EP simply absent off-macOS. **Path B (direct Core ML via `objc2-core-ml`) only if A fails the criteria below** — a committed fallback, not a redesign discussion.
- **Two artifacts, one per execution provider, selected by the manifest's `platform`/`key` (2c):** **fp32 ONNX for the CPU EP** (`tozp/laya-onnx` `model.onnx`, single file, opset 14; outputs may be named `act` instead of `act_logits` — `verify_graph` accepts both) on every non-Apple-silicon platform; **fp16 ONNX for the CoreML EP** on `aarch64-apple-darwin` — fp16 *is* the GPU/ANE artifact (the community files it as GPU-only), it halves the ORT-side copy and spares Core ML a fp32→fp16 conversion at compile. A Mac where the CoreML EP fails to register falls back to fp16 on CPU (slow) with a loud warning — acceptable: every Apple-silicon Mac has Core ML. On the CPU EP the graph runs with dynamic shapes (no padding cost); **bucketing to S = 512 / K = 32 is applied only when the CoreML EP is active** (accelerators want static shapes).
- Pass criteria for A, measured by the bench on Sky's M3 Max, fp32 root, S = 512 / K = 32, model-cache dir set, warm: **≤ 3 CoreML partitions** (from ORT's `GetCapability` log line), **p95 ≤ 50 ms** per single question, **RSS ≤ 2 GiB** after the first inference with the fp16 artifact (Sky's correction: ORT keeps the ONNX initialisers *and* Core ML holds its compiled copy, so the floor is two copies of the weights — ≈ 0.85 + ≈ 0.85 GiB for fp16; a 1.69 GB fp32 file can never sit under 1 GiB; the sub-1 GiB figures in §12.2 are native Core ML with mmapped weights) — report the breakdown after load / after compile / after first run, **argmax parity with the CPU EP fp32 run** on the README examples and the model-gated tests, and a **cold first-load (compile) time recorded** and cached across runs. Fail any one → Path B with `FluidInference/laya-coreml`-style buckets converted from the English root in CI (no pip on Sky's Mac: every export we make runs in GitHub Actions).
- v1 meeting briefs are **not blocked** on the spike: the fp32 CPU path (official 193–464 ms) is acceptable at batch cadence and is what ships if the spike drags.

### 12.7 Step 2d — Core ML spike (before publishing)

1. Add the **fp32 root ONNX** (tozp) beside the fp16 one; bench fp32 on the CPU EP with dynamic shapes — the honest CPU baseline (expect ~200–450 ms, ~2 GiB RSS: that is what CPU-only platforms pay) — and also with S = 512 / K = 32 padding to see the padding cost.
2. Register the **CoreML EP** in `ort` (`MLProgram`, model-cache dir, compute units all / CPU+ANE), bucket to S = 512 / K = 32; run the **fp16** artifact (the candidate) and the fp32 one (data point); capture the partition count from ORT's log; p50/p95, RSS breakdown, cold compile time; argmax parity vs step 1.
3. Apply the §12.6 criteria. Pass → the shipped macOS path is ORT + CoreML EP, fp32 artifact, one download. Fail → **stop and report**; Path B is briefed separately.
4. Only then run 2b. Record everything in the as-built.

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

## As built — step 2c (2026-09-21)

Brief: `/tmp/laya-design/omp-step2c.md`, Sky's direction: the ONNX Runtime is bundled by the app build, uploaded to the model bucket so it can be placed anywhere, and "putting it next to parakeet makes no sense". Commits `fc35712` (`ort_lib(data_dir)` + every caller), `7286f66` (pipeline + desktop), this note.

### What landed

- **One on-disk home**: `<data_dir>/models/onnxruntime/libonnxruntime.{dylib,so}` (`onnxruntime.dll` on Windows — Microsoft's own name; the brief's `libonnxruntime.dll` does not exist), unversioned, plus `VERSION` (`1.27.0`). `models::onnx::{DIR, LIB, runtime_path}` name it. Nothing else under `models/` is a runtime.
- **`models::onnx::ort_lib(data_dir: &Path) -> Result<PathBuf, String>`**, no hint dirs: 1 bundle (`Contents/Frameworks/<LIB>` or beside the executable — first because a hardened-runtime app must dlopen the copy we codesigned), 2 `<data_dir>/models/onnxruntime/<LIB>`, 3 `$ORT_DYLIB_PATH`, 4 Homebrew `/opt/homebrew/lib`, `/usr/local/lib`. The hit is logged at debug with its source; `ort::init_from(..).commit()` still runs once per process. **Deviation**: the `OnceLock<Result<..>>` became a `Mutex<Option<PathBuf>>` — a failed search is *not* cached any more, because the desktop downloader may land the dylib while the daemon runs and the next `Ask`/dictation must find it (the old code pinned the first failure for the process lifetime). The error text still starts `no libonnxruntime.dylib …` so the existing `contains("libonnxruntime")` test guards hold, and now names location 2 as the place to put it. `ort_ready()` kept.
- **Callers pass the data dir**: `voice::Engine::load(dir, data_dir)`, `Stream::open(dir, data_dir)`, `live::Utterer::open(dir, data_dir)` (`Transcriber` and `Dictation` carry `rt.data_dir`); `meeting::post::Paths.data_dir`, `diarize::Models.data_dir` (`diarize::models(dir, data_dir)`, `Embedder::open(&Models)`, `speech(.., data_dir, ..)`), `Setup.data_dir` for the live sessions; `decision::laya::Loaded::open(dir, data_dir, threads, coreml)`, `Engine::new(cfg, model_dir, data_dir, log)`, `Loader = Fn(&Path, &Path, &DecisionConfig)`. The `dir/../parakeet-tdt` hint from step 2 is gone. Generated-config comment for `voice.stt_model_dir` now describes the shared runtime and its location.
- **`decision::laya::verify_graph` takes `(String, ValueType)` pairs** instead of `ort::value::Outlet`s: `Outlet::new` dlopens the runtime through ort's default path (it panicked in the release test run where `libonnxruntime.dylib` was not on the loader path), so the pure unit test must not construct one. `Loaded::open` maps `session.inputs()/outputs()` into pairs.
- **`scripts/ort-version.env`**: `ORT_VERSION="1.27.0"`, `ORT_SHA256_OSX_ARM64`, commented `LINUX_X64` / `WIN_X64` placeholders for 58. `package-macos.sh` sources it (inline pin removed).
- **`release-models.sh`**: header rewritten (the "libonnxruntime is not a model" line is gone); `PLATFORMS=(osx-arm64)` with `triple_of` / `lib_of` / `member_of` / `sha_of` mapping tables (`osx-arm64 → aarch64-apple-darwin → libonnxruntime.dylib`, rows for `linux-x64` and `win-x64` ready for 58); after the models it fetches Microsoft's archive into `target/` (cached, sha256-checked, same as `package-macos.sh`), extracts the bare dylib, uploads `onnxruntime/<triple>/<lib>` and `onnxruntime/<triple>/VERSION`, and adds manifest rows with `key` and `platform`; `upload`/`manifest_add` helpers replace the duplicated loop bodies; the verify step HEADs the runtime keys too. Model rows are byte-identical to before (no `platform`, no `key`).
- **`crates/desktop/src/models.rs`**: `Entry.platform: Option<String>`, `Entry.key: Option<String>`; `for_this_platform()` against a `cfg!(target_os, target_arch)`-derived `TARGET_TRIPLE` const (chosen over a `build.rs` exporting `TARGET`: the platforms we ship are unambiguous from those two, and it is one expression instead of a build script; `match` on `&str` in a `const` is unstable, hence the `if cfg!` chain); download from `key`, land at `path`, both checked for `..`; the runtime gets `chmod 755` after the sha256 check (cfg(unix)). Module doc says "Model files and the ONNX Runtime the daemon loads". New unit test `platform_and_key_fields`.
- The user's old `~/.theuth/models/parakeet-tdt/libonnxruntime.1.27.0.dylib` is **not deleted**; `ort_lib` simply no longer looks there — remove it by hand when convenient. Note: that copy is 29.6 MB, Microsoft's 1.27.0 release dylib is 38.3 MB (sha256 `299e5a2c…`); the canonical location now holds Microsoft's.

### Checks

- `cargo check --workspace --all-targets`: green.
- `cargo test --release -p models -p voice -p meeting -p decision`: decision 30 + 2 (model-gated, loading the runtime from `~/.theuth/models/onnxruntime/` — the only copy `ort_lib(~/.theuth)` can reach on this machine: no bundle, no `$ORT_DYLIB_PATH`, no Homebrew), meeting 32, voice 9 (the TDT/streaming tests ran against the same copy), models 13; 0 failed. `cargo test -p desktop models::`: 2 passed.
- `cargo clippy --no-deps --all-targets` on models, voice, meeting, decision, desktop: 0 warnings. `bash -n scripts/package-macos.sh scripts/release-models.sh`: ok.
- Dry run — `.env.signing` is not present on this machine, so the AWS variables were placeholders (`AWS_ACCESS_KEY_ID=dry AWS_SECRET_ACCESS_KEY=run THEUTH_S3_DRY_RUN=1 bash scripts/release-models.sh`); every `head-object` therefore fails closed to "upload", which is what a dry run should show. Real output (full copy in `/tmp/laya-design/release-dry-run.txt`):

  ```
  ==> models from /Users/Sky.Moore/.theuth/models → s3://theuth/releases/models/ (public https://s3.rwx.dev/theuth/releases/models/, dry run: 1)
      pocket-tts/tts_b6369a24.safetensors: upload (235738732 bytes)
      DRY_RUN: aws s3 cp /Users/Sky.Moore/.theuth/models/pocket-tts/tts_b6369a24.safetensors s3://theuth/releases/models/pocket-tts/tts_b6369a24.safetensors
      … (13 more model files, incl. decision/model.onnx 846269825 bytes) …
  ==> ONNX Runtime 1.27.0 → s3://theuth/releases/models/onnxruntime/<triple>/
      fetching onnxruntime-osx-arm64-1.27.0.tgz
      onnxruntime/aarch64-apple-darwin/libonnxruntime.dylib: upload (38313360 bytes)
      DRY_RUN: aws s3 cp …/aarch64-apple-darwin/libonnxruntime.dylib s3://theuth/releases/models/onnxruntime/aarch64-apple-darwin/libonnxruntime.dylib
      onnxruntime/aarch64-apple-darwin/VERSION: upload (7 bytes)
      DRY_RUN: aws s3 cp …/aarch64-apple-darwin/VERSION s3://theuth/releases/models/onnxruntime/aarch64-apple-darwin/VERSION
  ==> verify via https://s3.rwx.dev/theuth/releases/models/
      DRY_RUN: nothing to verify
  ==> models.json
  { "files": [ … 14 model rows unchanged …,
      { "path": "onnxruntime/libonnxruntime.dylib", "size": 38313360,
        "sha256": "299e5a2c6ea00531ecd6bf3217e23798c1fcba1698bb386d313c8e16d7317d60",
        "key": "onnxruntime/aarch64-apple-darwin/libonnxruntime.dylib", "platform": "aarch64-apple-darwin" },
      { "path": "onnxruntime/VERSION", "size": 7,
        "sha256": "7ef1ea58fece676ff7345f6edac427e671daf20f0d7499ef2e42ada241d4fe24",
        "key": "onnxruntime/aarch64-apple-darwin/VERSION", "platform": "aarch64-apple-darwin" } ] }
      DRY_RUN: aws s3 cp …/models.json s3://theuth/releases/models/models.json
  ==> done: https://s3.rwx.dev/theuth/releases/models/models.json
  ```
  Not run for real: nothing uploaded. `just release-models` with the real `.env.signing` is Sky's call (57 §6 step 2b covers the checkpoint at the same time).

### Open

- The bundle's `Contents/Frameworks/libonnxruntime.dylib` and the bucket copy are the same Microsoft artifact by construction (same `ort-version.env` pin), but nothing checks the installed `models/onnxruntime/VERSION` against the pin the binary was built with; a mismatch is only visible in the debug log line. Add a warning if it ever bites.
- Homebrew (location 4) may hold a *different* ORT version; it is last for that reason.
