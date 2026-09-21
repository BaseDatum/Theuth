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

### 3.4 Operating switch and visibility (Sky, 2026-09-21 night)

Sky wants to run meetings in both configurations and *see* what the oracle is doing, so the meeting family gets an owner-facing switch and live UI, not just a config key and a log file.

**Switch** — Desktop Settings → Meetings → "Local decision oracle": a three-way segmented control **Off · Shadow · Active**, default **Shadow** once the model is installed (Off when `ModelMissing`, with the reason shown).
- **Off**: the oracle is not asked; today's path exactly.
- **Shadow**: the oracle is asked, its disposition is recorded and *shown*, and today's model turn still runs; after the turn the plugin reports `Outcome` (agreed / model did X) so the same batch shows both answers side by side.
- **Active**: the oracle gates the turn as in §3.2 (Accept → no model turn; Escalate → tiers 2/3).
It is the `decision.families.meeting.*.mode` setting (§2) for both meeting families at once, written through the same config RPC the model pickers use (runtime value + `config.yaml`), effective from the next batch — no restart. The TUI keeps `/decision` for the numbers; the switch is the day-to-day control.

**Session view** (the meeting session's chat, where batches are steered today): every batch that went through the oracle gets one compact status row under it — `⚡ topic 0.12 · skip · 78 ms` / `⚡ brief SRE-2386 0.91 · published · 81 ms` / `⚡ escalate · low margin 0.55 / 0.31 · tier 2 → skip` — and in Shadow the row gains the outcome after the model turn: `✓ agreed` or `✗ model published HR-12`. Rows are dim, single-line, expandable to the full probabilities. In Off nothing is added.

**Island**: a small state chip in the island header next to the clock: mode badge (`shadow` / `active`; none when Off) and a live indicator that steps `⚡ deciding` (the ~80 ms flash) → `skip` (dim, fades) / `brief` (the topic row appears with the `⚡` source marker) / `→ model` (the existing thinking indicator continues). Session counters live in the chip's tooltip and in the Meetings tab header: `⚡ 14 skipped · 3 briefs · 2 escalated · agreed 92 %`.

**Data path**: the meeting plugin emits `OracleDecision { batch_id, family, mode, disposition, top: [(label, p)], latency_ms, tier, outcome: Option<Outcome> }` on the meeting event stream the island already subscribes to (topics, level, state), `ts_rs`-exported like `MeetingState`; `store/meeting.ts` keeps the last N per meeting and the counters; `Island.tsx` and the session batch rows render from the store. The row and the chip are the only new UI; no new window, no new RPC beyond the config write.

### 3.5 Revision after the first shadow session (Sky, 2026-09-21 evening): the topic question is broken; tier 2 becomes the workhorse

**Evidence** (`~/.theuth/decisions/2026-09-21.jsonl`, 12 batches in shadow): `meeting.topic` answered **`false` on every batch**, with logits 12–21 against ≈0 (P(true) ≤ 0.001) — including the Kuiper-belt / Planet Nine narrative and four batches for which the model *did* publish a brief (`corrected: actual true` ×4). `meeting.brief` over 8–9 cached HV/MV titles gave a flat 0.05–0.24 spread every time (`low_top1`), `none` correctly on the astronomy batch. Two first-batch asks timed out at 401 ms while the model cold-loaded (5.1 s), so the first batch of a meeting always went to the model. Conclusion: the yes/no question as phrased is not "uncertain", it is *confidently wrong* — the gate cannot rescue it. Causes (§12.4's `laya-grounded` findings apply): the statement is about the *listener* ("…the listener may need to look up") — an abstract third-party judgement the model was never trained on; its vocabulary ("ticket, system, project, number") primes an IT context; `noul` grounding is the base checkpoint's weakest skill while routing `Choice` is its strongest; 9-way choices over long, near-identical titles flatten.

**Sky's requirements**: identify the topic really well; match an existing brief when one exists; escalate when not; **never ask the classifier about the user** (it knows nothing about them); several decision steps are fine; **1–2 s per batch is fine**.

**Redesign** — the classifier does what it is good at (recognise, route, match short labels); a cheap LLM (tier 2, `agent.escalate_model`) does what needs reading comprehension (name the subject); the reasoning model (tier 3) does what needs tools (research):

0. **Mentions (deterministic, 0 ms).** From the batch text: capitalised n-grams not at sentence start, ticket ids (`[A-Z]{2,}-\d+`), acronyms, numbers with units, quoted terms, proper names → `mentions: Vec<String>` (`Kuiper belt`, `Mike`, `Constantin`, `Planet Nine`). Also the FTS query for cached briefs is now built from mentions ∪ batch text, and candidates are cut to the **top 4** by FTS rank + `none`.
1. **Tier 1a — conversation kind (`Choice`, family `meeting.kind`)**: *"What is this stretch of conversation mostly about?"* → `small talk, greetings or scheduling` · `a specific named subject (a person, place, object, ticket, system, event or technical term)` · `general discussion without one specific subject`. The exact phrasing is chosen by the phrasing bench below, not by hand.
2. **Tier 1b — cached brief (`Choice`, family `meeting.brief`)**: unchanged question, ≤ 4 candidates + `none`, labels = titles only.
3. **Gate (asymmetric — a missed brief costs more than a wrong cached one):**
   - **Skip** only if 1a = small-talk with P ≥ τ **and** `mentions` is empty. Either condition alone → not a skip.
   - **Publish** if 1b picks a brief with P ≥ τ_brief (start 0.6) and margin ≥ 0.3 over the runner-up.
   - **Everything else → tier 2**, always (shadow included — Sky accepts the cost; shadow must evaluate the whole pipeline).
4. **Tier 2 — subject extraction (`decision::escalate`, budget 3 s)**: one JSON answer `{ "subject": string|null, "cached": <brief id>|null, "lookup": bool }` from the batch text + `mentions` + the ≤ 4 cached candidates (id, title, aliases) + tier 1's numbers as a data line. `cached` → publish that brief (same dedupe). `subject` with `lookup: true` → **tier 3 with the subject as the lead**: the evidence line becomes `oracle: subject "Planet Nine / Kuiper belt" — no cached brief; kind: named subject 0.91` so the research turn starts from the answer, not the transcript. `subject: null` → skip (carried). Invalid/timeout → tier 3 as today.
5. **Warm-up**: when a meeting reaches `Recording`, the plugin sends a warm `Ask` so batch 1 never times out on the cold load.

**Phrasing bench** (before wiring): `decision` example `phrasing` replays fixture batches (built from today's recorded meetings' transcripts in the meeting store, labelled by what the model published per `briefs.jsonl`/outcomes, plus a handful of hand-written small-talk and narrative batches) against candidate phrasings/label sets for 1a and 1b, printing per-phrasing confusion, mean P on the right label, and how many skips would have hidden a published brief. The winner goes into `oracle.rs` constants with the bench's table in the as-built.

**UI**: the ⚡ row shows the stages: `⚡ kind: named subject 0.91 · mentions: Kuiper belt, Constantin · brief: none 0.57 · tier 2: "Planet Nine / Kuiper belt" → no cached brief → model`. Counters gain `tier 2 calls`.

## §4 Second integration: browser (23)

Today the model lists elements (`browser_elements` / `browser_read`), reads `ref=e3 button "Sign in"` lines, then acts — two turns per action, the second one containing the whole element list. `browser_do(intent, operation?)` (Mutating, like `browser_click`):

1. Snapshot elements (existing `session.rs`), pre-rank to ≤ 10 candidates with the `search` crate's fuzzy match against `intent` (laya's 512-token budget cannot take hundreds of elements), add `none`.
2. `Ask` batch: `Choice(operation ∈ {click, fill, select, press, wait, none})` + speculative `Choice(target)` — one forward pass, jev's shape. Family `browser.target`.
3. **Accept** → execute the existing tool path for that ref (same navigation wait, same post-action snapshot, same risk/policy — `browser_do` declares `Risk::Mutating`, so policy asks exactly as it would for `browser_click`). Reply = today's click reply + `chose e3 (0.94) over e7 (0.04)`.
4. **Escalate** → tier 2 inside the same tool call: the cheap model picks from the same ≤ 10 candidates; a valid ref executes as in step 3 with `tier: 2` in the reply. Only `none`/invalid/timeout falls through: the reply is the candidate list with probabilities and the reason — i.e. what `browser_elements` returns today plus evidence — and the model picks in this same turn with `browser_click`. Failure mode = current behaviour, one extra local call.
5. `browser_wait` condition selection is a second, low-risk family (`browser.wait`) inside the same tool: `Choice` over `{text: "<visible heading>", selector: …}` candidates derived from the intent.

Not changed: refs stale after navigation still fail closed; `site_learn`/routes stay heuristic (`routes/src/classify.rs`) — they are already local and deterministic.

### 4.1 Revision (Sky, 2026-09-21 night): the browser System-1 loop — the jev-ultrafast demo, in-process

Sky's aim is the `cklxx/laya-browser` demo: laya drives a real browser end to end, **one encoder pass per step, no LLM in the loop** (their demo: 6 tasks, 12 decisions, median 22 ms/decision on an RTX 4070 through `browser-use/jev-ultrafast`). The §4 sketch above (a single `browser_do` inside a model turn) is superseded by a loop tool.

**Facts relied on** (`cklxx/laya-browser` README + `code/sample_request.json`, read 2026-09-21):
- Checkpoint **v10** = ModernBERT-large 421M, English, `model.safetensors` 842,609,220 B + `encoder/config.json` + `rl_agent_config.json` + `tokenizer/` — the exact layout `decision` loads; `head_max_len` 768. v10s/v11s are mmBERT-base 322M (multilingual, 17–23 ms on the 4070) — a different backbone the vendored port does not implement; later.
- Numbers: element top-1 on held-out pages 0.10 (base) → **0.66** (v10) over ~45 candidates; operation accuracy 0.54 → 0.88; 16 real tasks × 3 runs: v10 50 %, v10s 62 %, bimodal (10 pass 3/3, 6 fail 3/3 — login impossible because the DOM reader hides password fields; collapsed menus unseen). **Confidence-gated escalation to Qwen3-8B made it worse (58 → 42 %)** — no tier 2 in this loop.
- Request shape (jev `/v1/systemone`, = laya's `predict` request): `state = { page: { url, title, text ≤ ~1.5k chars }, recent_actions: [] }`; questions `operation` (Choice; criteria **verbatim**: `CLICK`, `TYPE_TEXT` ("…A small LLM will supply the value from the goal."), `SCROLL_DOWN`, `WAIT`, `DONE`, `BLOCKED`; instructions `{ goal, rules }` — a fixed ~12-line rule text), `click_target` (Choice over every interactive element as `"[i] label (role)"`, instructions `{ goal, operation: "CLICK", rules: [rules, target-rules] }`), `type_text_target` (same over editable elements). Elements live **only in the option list**; the fine-tune learned this rendering — drift here is the fastest way to lose the 0.66.

**Design**
1. **Tool `browser_run(goal, max_steps = 25)`** (Mutating; the description says it may perform up to `max_steps` clicks/keystrokes in the session's headless browser under one approval): the loop runs inside the tool call. Per step: `observe()` → one **`AskMany`** (the three questions in one collate + one forward; new engine message — today `Ask` is one question and the meeting oracle pays two round trips) → gate → execute → append to `recent_actions` → repeat. Returns a trace (`step 3: CLICK [7] "Search (button)" 0.91 · 180 ms`), the final url/title and why it stopped.
2. **`browser::systemone::observe(session) -> Observation`**: url, title, visible text in DOM order cut to 1.5k chars, interactive elements in DOM order `{ index, ref, label, role, value?, editable }` (links, buttons, inputs, selects, textareas, `[role=…]`, `[onclick]`; `label` = accessible name → text → placeholder → aria-label; role in jev's words `link/button/searchbox/textbox/checkbox/combobox/…`; **password fields excluded**, as jev), `recent_actions` (last 6: `op`, target label, ok/err). Built on `session.rs`'s existing element snapshot (`ref=e3 …`) — the `[i]` index maps back to the `ref`.
3. **TYPE_TEXT value**: deterministic first (a quoted string in the goal, or the goal's object after "for/into/as"); otherwise one `SideCall{ role: Escalate }` with goal + field label + page title → ≤ 80 chars. That is the only LLM call the loop can make, and only when typing.
4. **Gate and stop rules** (`browser.operation`, `browser.target`, `browser.type_target` families; modes off/shadow/active): execute when `operation` top-1 P ≥ 0.5 and the target's P ≥ 0.25 (cklxx shipped their suite ungated and at 0.7 — both thresholds are config, the bench decides); `DONE` ends the loop; `BLOCKED`, step cap, 60 s wall clock, or **three identical consecutive actions** (the README's failure signature) hand back to **tier 3**: the tool returns the observation + trace and today's `browser_*` tools continue in the same model turn. **No tier 2 for op/target** (their finding).
5. **Shadow** = `browser_run` observes and asks but does not execute; the tool result is the trace prefixed to what `browser_elements` returns, so the model's actual next `browser_*` call is the label; agreement is logged like the meeting outcome. **Active** executes. Off = tool hidden.
6. **Checkpoint per family**: `decision.checkpoints: { default: models/decision, browser: models/decision-browser }`, `families.browser.*.checkpoint: browser`; one engine actor per checkpoint, both may be resident (~0.9 GB each on Metal; idle unload as today). `models/decision-browser/` = cklxx v10 files; fetched by `release-models`/the desktop downloader once published — until then a `scripts/fetch-laya-browser.sh` from Hugging Face.
7. **Capture** (§11.4 shape, family `browser`): per step the observation, the three questions with candidates, probabilities, the executed action, and the loop's end state; the model's tier-3 correction, when it takes over, is the gold label (DAgger, which is exactly what cklxx used for their last 177 examples).
8. **Bench** = cklxx's 16-task suite (`results/suite_*.json` on the hub has the task list) × 3 runs, pass/fail + steps + per-step latency, run against public sites from `examples/browser_suite.rs`; the gate thresholds and the "match jev's reader" claim are judged by it. Target on the M3 Max: ≤ 300 ms per step at ~45 candidates; success ≥ their v10 50 % on the same tasks.

Not in scope: mmBERT (v10s), computer use (needs an AX-tree reader — §4.2 later), the Settings pane (browser mode is a config key until step 10 groups the switches).

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

- Meeting island: an escalated batch shows nothing new; an oracle-accepted brief shows exactly like an alias hit. A small `⚡` marker on oracle-published topics (53's topic row already carries a source enum: add `Oracle`). Plus the §3.4 state chip (mode badge + deciding/skip/brief/→ model indicator, counters in the tooltip).
- Desktop Settings → Meetings: **Local decision oracle** Off · Shadow · Active (§3.4) — the day-to-day switch; the meeting session shows one `⚡` status row per oracle-handled batch, with the shadow outcome appended after the model turn.
- Desktop Settings → Agent loop: **Escalation model** picker (`agent.escalate_model`), same control as the title/vision model pickers, placeholder "inherits title model"; unset by default.
- Desktop Settings → "Decision training data": per-family opt-in switches (§11.1.2), off by default.
- TUI `/decision` (or `Stats` via RPC `decision_stats`): per family — mode, coverage, escalations by reason, p95, shadow agreement. That is how Sky decides to flip `active`.
- Config comments in `theuth/src/main.rs` generated config: model dir, that the desktop app fetches the checkpoint from the release bucket (54 §9), the shadow→active procedure in two lines.

### 7.1 Settings → Labs (Sky, 2026-09-21 night) — replaces step 10's "Decision training data" section

Everything experimental about the decision oracle lives in **one Settings pane, "Labs"**, so a user finds the switch, the consequence and the consent in the same place. Three cards, each with a one-paragraph plain-English description (what it is, what it does on this machine, what it costs) and a "design notes" link (this doc):

1. **Meeting oracle** — Off · Shadow · Active (`decision.families.meeting.mode`; the control moves here from Settings → Meetings, which keeps a one-line pointer "Local decision oracle: Shadow — change in Labs"). Below it the live counters (`decision_stats`): batches · skipped · briefs · tier-2 calls · agreed %. Disabled with the reason when `effective = off` (model missing, decision disabled).
2. **Browser System-1** — Off · Shadow · Active (`decision.families.browser.mode`). Text states that Active lets `browser_run` perform up to `max_steps` actions in the headless browser under one approval, the suite pass rate (56 %) and the step latency, and that Shadow only annotates. Disabled with the reason when the `decision-browser` checkpoint is missing, with a **Download (0.8 GB)** action wired to the model downloader once the checkpoint is published (until then the reason names `scripts/fetch-laya-browser.sh`).
3. **Training data** — per family (meeting, browser), two switches, deliberately separate:
   - **Capture locally** (`families.<p>.train`): exactly what one row contains, in the family's own words (meeting: "the last transcript batch — other people's speech — the classifier's numbers, what the assistant did next, and whether you tapped or dismissed the brief"; browser: "the page's element list and visible text, the goal, and the action taken"); examples today / total, bytes, **Open folder**, **Delete local data** (removes `decisions/training/<p>/`, keeps the switch as set).
   - **Share with theuth** (`families.<p>.share`, new key, default false): consent text — the uploaded rows are byte-identical to the local ones; no session, meeting or account identifiers; per-harness daily cap; revocable (stops uploads, cannot recall what was sent); link to the privacy note. Requires Capture on (turning Capture off turns Share off). **Until step 11's relay endpoint ships the switch is shown, persisted and greyed "upload not available in this version"** — consent recorded now is honoured by the uploader later, and the uploader must re-read the key on every batch.

Rules: every control is a live config write through the existing RPC pattern (`set_meeting_oracle_mode`, generalised to `set_decision_mode(prefix, mode)`; `set_decision_training(prefix, on)`; new `set_decision_share(prefix, on)`), no restart; each mode control shows its *effective* state; nothing in Labs defaults on except meeting Shadow (already the default). The pane is hidden entirely when `decision.enabled: false`.

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

### 11.4 Revision for steps 8–9 after §3.5 (Sky, 2026-09-21 night): local capture first, labels are free, no composer call

The pipeline of §3.5 changes what capture needs. Every non-skipped batch already gets a **tier-2 answer** (`subject`, `cached`, `lookup`) in shadow and in active, and every tier-3 delivery already produces an **outcome** (what the model published, or nothing); the user's **tap / dismiss** on a topic arrives later on the same brief. Those three are the labels — the §11.1(4) composer side call is unnecessary for the meeting family and is **dropped from v1** (kept in the design as an option for a family with no tier-2 pass). Fine-tuning targets are the two tier-1 questions the bench showed weak (`meeting.kind`, `meeting.brief`) plus, later, a `meeting.subject_present` Bool the fixture can already label.

1. **What one example is** — one per batch that entered the oracle (mode ≠ off) while `decision.families.meeting.train: true`: the batch text as the oracle saw it (`them:`/`me:` lines, ≤ 512-token truncation as applied), `mentions`, the rendered tier-1 questions (instructions, candidate ids *and titles*, `none`), tier-1 probabilities and dispositions, tier-2 `{subject, cached, lookup}` (or the failure), tier-3 outcome (`published: <brief id, title> | none`), and — appended later — `user: tapped | dismissed | ignored`. Labels are derived offline: `kind = small_talk` ⇐ tier 2 `subject: null` and nothing published; `named_subject` ⇐ a subject or a publication; `brief = <id>` ⇐ cached pick confirmed by publication (or tapped), `none` otherwise. **Tier-1 `Accept` rows are captured too** (a skipped small-talk batch is a negative example) — §11.1(3)'s "never capture Accept" was written for a world where the oracle's answer was the only label; now the label source is recorded (`label.source ∈ {tier2, tier3, user, oracle_only}`) and the trainer chooses.
2. **Where** — `<data_dir>/decisions/training/<family>/<YYYY-MM-DD>.jsonl`, one JSONL row per event: `example` (written when the batch settles) and `amend` (outcome, user reaction — keyed by the example `id`, never rewriting the file). Secrets scrubbed with the `secrets` crate's placeholder scrubber before writing. No session id, no meeting title, no participant names beyond what the speech contains.
3. **Opt-in** — `decision.families.<prefix>.train: bool` (default false; prefix patch as for `mode`), engine `GetTrain`/`SetTrain` messages, RPC `decision_training(prefix) -> { enabled, examples_today, examples_total, bytes, dir }` and `set_decision_training(prefix, bool)`; Settings section is step 10 — until then Sky flips it in `config.yaml` or over RPC.
4. **Export** — `t decision export --family meeting [--since DATE] --out FILE`: folds `example` + `amend` rows into the **same fixture shape the phrasing bench reads** (`crates/decision/tests/fixtures/meeting-batches.json`: `text`, `kind`, `published`, `mentions`, plus `subject`, `cached`, `user`) and, with `--format laya`, into laya's `typed-decisions` training JSON. The bench thereby becomes the evaluation harness for captured data: `cargo run --example phrasing -- --fixture exported.json`.
5. **Upload** (step 11, unchanged design) stays off until the relay endpoint exists; `train_keep_local` defaults true.

**As built** (`52e6e7b`, `87de365`, `2795f84`): see *As built — steps 8–9* at the end — the export flag is `t --decision-export meeting` (no subcommand tree in `t`), the example carries a `verdict`, ids are uuid v7, and a tier-1 skip without tier 2 labels `small_talk` too.

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

### 12.8 Step 2e — candle spike (Sky, 2026-09-21 night): two native Rust ports appeared

Path A failed (as-built 2d). Before Path B, two independent **candle** ports of laya surfaced (both Apache-2.0, both load the `safetensors` checkpoint directly — no ONNX, no coremltools, no export pipeline, no dylib): `laya-rs` 0.1.0 on crates.io (bob-rietveld; port of laya-mlx; CPU/CUDA/**Metal**; parity tests vs a NumPy transcription on a synthetic checkpoint; GitHub repo 404 — vendor the crate source) and `apiplant/laya-rs` 0.2.0 on GitHub (from scratch; CPU/CUDA/flash-attn, **no Metal**; RTX 4090 16-question fixture 135 ms vs PyTorch 165 ms; fixed candle slow paths; also an RLCD trainer in Rust — relevant to §11 later). §0.2 rejected candle for the porting cost; that cost is now zero. Sky: build it.

**Step 2e**: `laya-rs` 0.1.0 with `metal` as a third backend behind `Oracle` (`crates/decision/src/candle.rs`), driving its `DecisionModel` forward with *our* collated tensors so `build_sequence`, calibration, gate and log are unchanged; root `model.safetensors` fp16 on Metal, f32 on CPU. Criteria (same shape as §12.6): **p95 ≤ 50 ms** warm single question on Metal (S ≈ 340–512, K 2–10, no bucketing), **RSS ≤ 1.5 GiB** after first inference (one mmapped fp16 copy), **|Δp| ≤ 0.01** vs the ORT fp32 CPU probabilities (`/tmp/laya-design/probs-cpu-fp32.json`), cold load ≤ 3 s. Pass → candle is the decision engine on every platform (Metal / CUDA / CPU), the ONNX artifacts and ORT-for-decision go in a follow-up, fine-tunes become directory drop-ins. Fail → Path B as planned; the CPU numbers still inform 58.

**Outcome (Sky, 2026-09-21 night, before the as-built landed):** Metal fp16 measured 74–111 ms p50 per single question (S 339–512), **72 ms for the README 4-question batch**, 903 MiB steady (1.68 GiB transient during load), parity max |Δp| 0.0006, cold load 2.0 s of which 1.86 s is our own sha256. Latency misses the literal 50 ms criterion by 1.5–2× — but fp16 ≈ f32 and 4 questions ≈ 1 question, so the cost is candle's unfused Metal launch overhead, not compute; the meeting batch (several questions per `Ask`) lands in ~70–100 ms against 2.8 s on ORT CPU. **Sky's decision: candle on Metal is the v1 engine regardless of the criterion.** Battery is explicitly not a goal ("plug it in — this is a high-performance tool, not a battery-sipping passive system"), so **Path B (Core ML / ANE) is dropped**, not deferred. Follow-ups, in order: (1) the sha256 leaves the hot load path (verify once at download, or hash in the background) → ~0.2 s load; (2) `decision.engine` default becomes `candle`, `device: auto` (Metal on Apple silicon, CUDA where built, CPU + Accelerate/MKL otherwise); the ORT path and the fp16/fp32 ONNX artifacts are removed from `decision` (ORT stays for parakeet); 2b publishes the safetensors checkpoint dir instead; (3) an optimisation pass on the Metal path with the apiplant tricks (explicit zero bias so LayerNorm takes the fused kernel, flattened GEMM, fused RoPE/GeGLU) with 50 ms as the target, not a gate. §12.6's two-artifact-per-platform scheme is void: one `decision/` checkpoint dir everywhere.

### 12.9 The other laya-rs (apiplant) — tracked, not adopted (Sky, 2026-09-21 night)

`github.com/apiplant/laya-rs` (framp; tags v0.1.0 / v0.2.0 / 0.3.0 within two days; not on crates.io — the name is taken by the port we vendored) is a **different, from-scratch** candle reimplementation: CUDA with fused NVRTC RoPE/GeGLU kernels, flash-attn varlen, unpadded packing, prescale-Q, the two backend-independent candle fixes (bias-free LayerNorm → fused kernel, 3D `Linear` → one GEMM), an **RLCD trainer in Rust**, wasm playground, Homebrew/apt/pacman packaging and release CI. RTX 4090: 16-question fixture 135 ms vs PyTorch 165 ms. **It has no Metal path** (`Device::cuda_if_available(0)` else CPU f32) — on Apple silicon today it is the CPU numbers of §12.8, so adopting it now would be a regression on the primary platform.

Decision: **vendor what we built on (crates.io `laya-rs` 0.1.0 model/weights/config, Metal-proven, parity-verified) into `crates/decision/src/candle/` now** (follow-up (1)); **track apiplant** — revisit when it reaches actual parity for us, i.e. a Metal device path with our README-example parity (|Δp| ≤ 0.01 vs `tests/fixtures/probs-cpu-fp32.json`) at ≤ the vendored port's latency on the M3 Max. Then consider switching it in as a fork + git submodule with a `path` dependency (it is maintained; ours is a 404 tarball), which would also bring the CUDA path for doc 58 and the trainer for §11. Until then, its two candle fixes are the template for follow-up (3), the Metal optimisation pass. Watch: `https://github.com/apiplant/laya-rs` releases; check quarterly or when 58's Linux CUDA work starts.

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

## As built — step 2d (2026-09-21): Core ML spike — Path A FAILS the §12.6 criteria; stopped before Path B

Brief: `/tmp/laya-design/omp-step2d.md` plus Sky's correction (`2469ae1`: fp16 is the CoreML EP input, fp32 the CPU input, RSS ≤ 2 GiB with breakdown). Commits `63c2568` (library: fp32/fp16 outputs, `act`/`act_logits`, bucketing, CoreML EP registration + fallback + partition capture), `20b2c59` (bench on both EPs, config comment), this note. The brief's first two commit subjects were merged by file rather than by feature: the fp32 support and the CoreML registration both live in `Loaded::open`, and an intermediate `cargo check`-green state would have meant writing the function twice.

### Artifacts

| | fp16 (candidate for the CoreML EP) | fp32 (CPU artifact) |
|---|---|---|
| source | `sevenreasons/laya-onnx-fp16` (step 2) | `tozp/laya-onnx` `model.onnx` |
| local | `~/.theuth/models/decision/model.onnx` | `~/.theuth/models/decision-fp32/model.onnx` (+ copies of `tokenizer.json`, `rl_agent_config.json`) |
| size / sha256 | 846,269,825 B, `1e0031fd826e…` | 1,685,917,998 B, `053ee547e0c67d43574e1f8c9f4fc4ff1e1f6fc48bf7c4de1b848a1d275bcbf1` |
| opset | 18 | 14 (TorchScript exporter, per the README) |
| inputs | `input_ids`, `attention_mask` `i64[batch_size, sequence_length]`; `marker_pos` `i64`, `marker_mask` `bool` `[batch_size, num_options]`; `qtype` `i64[batch_size]` | same names; dims `seq_len`, `num_markers` |
| outputs | `logits` `f16[batch_size, num_options]`, `act_logits` `f16[batch_size, 2]` | `logits` `f32[batch_size, Wherelogits_dim_1]`, **`act`** `f32[batch_size, 2]` |
| shapes actually accepted | fully dynamic | **S = 512 only**: `seq_len` is declared symbolic but the trace baked `Reshape_4` in every attention layer to `{512, 16, 64}`; S = 128/256/300 fail with `input_shape_size == requested_shape_size was false`. B and K are dynamic (B = 1/2 × K = 3/32 all run). |

The README (`/tmp/laya-design/ref/tozp-readme.md`) says only "Inputs: input_ids, attention_mask, marker_pos, marker_mask, qtype; Outputs: logits, act; opset 14; FP32 and INT8" — no preprocessing notes beyond the upstream card.

**Consequence for the engine** (`Provider::Cpu` in `laya.rs`): "dynamic shapes on the CPU EP" (§12.6) is not available with this artifact; `Loaded` on CPU pads S to the checkpoint's `max_len` (512) and leaves K dynamic — one rule that runs both files ("make the engine work with whichever file is in the dir"). Cost on the fp16 Mac-CPU fallback path: none measurable today (K = 2 / S = 339: 637 ms p50 padded vs 736 ms dynamic in step 2 — run-to-run noise dominates); fp32 pays nothing because it could never run unpadded. K-padding to 32 on top costs ~+40–100 ms on CPU (table below), so K stays dynamic on CPU.

### `verify_graph` / `run`

Outputs may be f32 or f16; the act head may be `act_logits` or `act`; anything else is refused and named (`expected f32|f16 rank 2`, `` no output `act_logits|act` ``). `Loaded::run` extracts by element type (`floats`: borrowed `&[f32]` for fp32, converted for fp16) and finds the act output by either name. `collate(rows, bucket)` pads at least to `(S, K)`; `Loaded.bucket` is `BUCKET = (512, 32)` on the CoreML EP and `(max_len, 0)` on CPU.

### CoreML EP in `ort` rc.13

`ort::ep::CoreML` is behind the crate feature `coreml` (`ort-sys/coreml = []`: option-string builder only, nothing to link; under `load-dynamic` registration is the generic `SessionOptionsAppendExecutionProvider("CoreML", keys, values)` on Microsoft's dylib, which ships the EP on osx-arm64). Options set: `ModelFormat=MLProgram`, `MLComputeUnits=ALL|CPUAndNeuralEngine`, `ModelCacheDirectory=<data_dir>/cache/coreml/decision/` (rc.13 exposes it as `with_model_cache_dir`; ORT 1.27 honours it — a 418 MB `…/3066006407364578280/2_dynamic_mlprogram/` tree appeared). `RequireStaticInputShapes` left at 0 as briefed. Registered first with `error_on_failure()`; the CPU EP is ORT's implicit last resort. Partition line: `SessionBuilder::with_log_level(Info)` + `with_logger(Arc<dyn Fn>)` (a per-session user logger, so the daemon's normal ORT logging is untouched); `parse_partitions` keeps the three integers; `Loaded.partitions`, logged at info as `CoreML EP took K of M nodes in N partitions`. If the EP registers but the session fails to build, `open` logs `CoreML EP unusable (<error>; GetCapability: Some((N, M, K)))` and rebuilds on CPU with `partitions = None`, `bucket = (512, 0)`.

### Measurements (M3 Max, 8 intra-op threads, ORT 1.27.0, 20 warm runs per row)

**CPU EP** (`--dump`/`--ref` parity: fp32 padded vs fp32 S-only 4/4 argmax, max |Δp| 0.0000; fp16 vs fp32 4/4 argmax, **max |Δp| 0.0002**):

| artifact / padding | cold load | RSS after load | first inference | RSS after first run | K2 S339 p50/p95 | K2 S512 | K5 S393 | K5 S512 | K10 S473 | K10 S512 | README 4-Q batch |
|---|---|---|---|---|---|---|---|---|---|---|---|
| fp32, S→512, K dynamic (**engine default on CPU**) | 4.8 s | 2309 MiB | 585 ms | **2313 MiB** | 581 / 585 | 590 / 605 | 627 / 647 | 662 / 685 | 679 / 692 | 679 / 695 | 2757 ms |
| fp32, S→512 K→32 (`--pad`) | 4.8 s | 2309 | 625 | 2312 | 680 / 696 | 696 / 715 | 703 / 725 | 717 / 740 | 719 / 744 | 722 / 748 | 3068 |
| fp16, S→512, K dynamic (Mac CPU fallback) | 2.2 s | 1167 | 681 | 2513 → **4065** peak | 637 / 696 | 733 / 780 | 775 / 830 | 738 / 793 | 741 / 750 | 758 / 776 | 3138 |

fp32 on CPU: ~2.3 GiB flat (weights once, no casts) — half the fp16-on-CPU footprint, as §12.1 predicted — and 580–720 ms per question, above the official 193–464 ms (PyTorch, presumably with MKL/oneDNN; ORT's fp32 CPU MatMul on Apple silicon is not that). Batching does not amortise on CPU (4 questions = 4.7× one).

**CoreML EP** — every attempt fell back to CPU; the rows below are the attempt, not a working accelerator:

| artifact | units | GetCapability | outcome | cold load incl. failed attempt | second run (cache 418 MB) |
|---|---|---|---|---|---|
| fp16 | All | — (never reached) | **ORT throws during session init**: `HandleNegativeAxis … axis 2 is not in valid range [-2,1]` (`onnxruntime/core/providers/common.h:32`) — a CoreML-EP op builder or shape pass on the fp16 opset-18 graph | 2.36 s | 2.34 s (nothing cached) |
| fp16 | CPU+ANE | — | same exception | 2.37 s | 2.35 s |
| fp32 | All | **`number of partitions supported by CoreML: 125 number of nodes in the graph: 2715 number of nodes supported by CoreML: 2150`** (verbatim values from ORT's INFO line, captured by `parse_partitions`) | Core ML **compile fails**: `Failed to create MLModel … Failed to build the model execution plan … model.mil with error code: -7`; E5RT: `Invalid tensor rank 0 inferred from: ios18.squeeze`, `Input _encoder_Mul_output_0 has unbounded dimension which is not supported` | 16.7 s | 15.0 s (cache does not help a failing compile) |
| fp32 | CPU+ANE | 125 / 2715 / 2150 | same | 12.8 s | 14.5 s |

Both models warn `CoreML does not support shapes with dimension values of 0. Input: /layers.N/self_attn/Slice_2_output_0, shape: {0}` — ModernBERT's rotary/unpadding slices produce empty tensors per layer, which is what fragments the graph into ~125 partitions (28 layers × ~4 cuts + head).

**Knob sweep** (throwaway probe, not committed; output in `/tmp/laya-design/coreml-probe.txt`):

| artifact | format | opt level | `RequireStaticInputShapes` | partitions / nodes / on CoreML | session |
|---|---|---|---|---|---|
| fp16 | MLProgram | 3 or 0 | 0 | — | init exception (above) |
| fp16 | MLProgram | 3 | **1** | 0 / 1622 / **0** | OK — nothing on Core ML |
| fp16 | NeuralNetwork | 3 or 0 | 0 | 0 / 1632 / **0** | OK — nothing on Core ML (the NN format has no fp16 path) |
| fp32 | MLProgram | 3 | 0 | 125 / 2715 / 2150 | compile fails (-7) |
| fp32 | MLProgram | 0 | 0 | 129 / 3284 / 2667 | compile fails (-7) |
| fp32 | MLProgram | 3 | **1** | 91 / 2715 / 591 | OK — 22 % of nodes |
| fp32 | NeuralNetwork | 3 | 0 | **224** / 2715 / 1502 | OK (8.2 s load) — 55 % of nodes in 224 pieces |
| fp32 | NeuralNetwork | 0 | 0 | 256 / 3284 / 1342 | OK (10.0 s) |

No configuration puts the encoder on Core ML in ≤ 3 pieces; the only sessions that build either run nothing on Core ML or split it into 91–256 partitions with the CPU EP shuttling tensors between them (latency not measured — the partition criterion already fails, and MLProgram is the format the design names).

### §12.6 verdict, criterion by criterion

| criterion | required | measured | verdict |
|---|---|---|---|
| CoreML partitions | ≤ 3 | fp16: session never builds; fp32: 125 (compile fails); best building variant 91 with 22 % coverage | **FAIL** |
| p95 warm, single question, S 512 / K 32 | ≤ 50 ms | no CoreML session exists to measure; CPU fallback 585–748 ms | **FAIL** |
| RSS after first inference, fp16 on CoreML EP | ≤ 2 GiB | not measurable (no session); breakdown for the record — fp16 CPU: 1167 MiB after load, 2513 MiB after first run, 4065 MiB peak; fp32 CPU: 2309 / 2313 / 2313 MiB | **FAIL** (not met by any running configuration) |
| argmax parity with CPU fp32 (README examples + model-gated tests) | all agree | fp16 CPU vs fp32 CPU: 4/4 argmax, max \|Δp\| 0.0002; `tests/model.rs` passes on both dirs (fp32 via `DECISION_MODEL_DIR`) | PASS (between the two CPU runs; there is no CoreML run to compare) |
| cold compile time recorded, cache makes the second load materially faster | recorded | fp32 attempt 12.8–16.7 s cold, 14.5–15.0 s with the 418 MB cache present — no improvement, because the compile fails every time; fp16 never reaches compile | **FAIL** |

**Path A fails.** Per the brief: `decision.coreml` default stays `false`, the shipped path for now is **fp32 on the CPU EP** everywhere (with S padded to 512, ~2.3 GiB, 580–720 ms per question on this machine), and this task **stops here — Path B (direct Core ML via `objc2-core-ml`, `FluidInference/laya-coreml`-style buckets exported from the English root in CI) is not started.** `coreml: true` remains available and safe: it warns and falls back.

### What the failures are (for the Path B brief / an ORT issue)

1. **fp16 + CoreML EP**: an exception inside ORT's own session initialisation, before Core ML is called — `HandleNegativeAxis(axis 2, rank 2)`. Deterministic across opt levels and compute units; goes away with `RequireStaticInputShapes=1` (which then takes zero nodes). Looks like a CoreML-EP op builder assuming rank ≥ 3 on one of the fp16 graph's rank-2 tensors — an ORT bug worth a minimal repro, not something the Rust side can work around.
2. **fp32 + CoreML EP (MLProgram)**: GetCapability succeeds (2150/2715 nodes) but Core ML's compiler rejects the MIL: rank-0 `squeeze` results and unbounded dimensions (`_encoder_Mul_output_0`, `_encoder_Reshape_1_output_0`) — the EP emits dynamic-shape MIL because the ONNX inputs are declared symbolic even though the trace is S = 512-fixed. **The one lever left for Path A** is a graph with static input dims (B = 1, S = 512, K = 32) baked into the ONNX file itself (onnx graph surgery or a re-export — a CI job, no pip on Sky's Mac); that would let `RequireStaticInputShapes=1` be meaningful and might collapse the partitions. Untested; it is the same export work Path B needs anyway, so it belongs in that brief as a first cheap check.
3. The 0-size `Slice` outputs per layer are inherent to the ModernBERT export and will fragment any ORT CoreML partitioning until ORT's EP handles them; Path B's own Core ML conversion (coremltools from PyTorch, as FluidInference did) sidesteps ORT's partitioner entirely, which is why their models are one partition at 5–18 ms.

### Also recorded

- fp32 CPU probabilities equal the step-2 fp16 numbers to 3 decimals (billing 0.965 / conf 0.86, urgency 1.44, churn 0.825): the fp16 export is faithful; the README's 0.94 / 1.84 / 0.892 belong to another checkpoint revision (step 2's open question stands, now with two independent exports agreeing).
- `scripts/release-models.sh` still lists `decision/model.onnx`; which file it uploads is whatever is in `~/.theuth/models/decision/` (today fp16). Per-platform selection of fp16 vs fp32 through the manifest's `platform`/`key` is 2b's job and is moot until an accelerator path exists — the CPU artifact is fp32 on every platform, including macOS, unless Sky prefers the smaller fp16 download for Macs at 2× CPU memory.
- Checks: `cargo check --workspace --all-targets` green; `cargo test --release -p decision`: 32 unit + 3 model-gated (fp16 dir) + 3 (fp32 dir via `DECISION_MODEL_DIR`); `cargo clippy -p decision --no-deps --all-targets` clean; the generated config comment for `coreml` states the fallback. Bench outputs: `/tmp/laya-design/bench-2d-{cpu-fp32,cpu-fp32-pad,cpu-fp16,coreml}.txt`, `fp32-shapes.txt`, `coreml-probe.txt`.

## As built — step 2e (2026-09-21): candle spike — laya-rs 0.1.0 on Metal works; latency misses the literal 50 ms, everything else passes; Sky accepts it as the v1 engine

Brief: `/tmp/laya-design/omp-step2e.md`. Commits `b31bae3` (backend: `candle.rs`, `Provider`-side wiring, `decision.engine` / `decision.device`, tests), `cae1745` (bench on Metal f16 / Metal f32 / CPU f32 with `--ref` parity), this note. Sky's decision on the numbers below is the **§12.8 Outcome** paragraph (main `aeda3ff`, written before this note landed); it is restated at the end.

### Checkpoint

`~/.theuth/models/decision-candle/`, the Hugging Face `convaiinnovations/laya` root (English) — the repo keeps `multilingual/` and `typed-decisions/` as subfolders; the root files are the English checkpoint:

| file | bytes | sha256 |
|---|---|---|
| `model.safetensors` (fp16) | 842,609,210 | `891102d372688fc2a094dac56a384bc537b87c63f21f9f3dac0be2b7cbc8d86c` |
| `encoder/config.json` | 2,083 | `bf3ab80598fdccf414855a2ce80f22859e4492d06ca8a62ddd1cfb63972f8979` |
| `rl_agent_config.json` | 745 | `ae287b56bbcf5f8c4f4541ae9dfd00c914c4c48b940b8398c3058af37ba92bbd` — byte-identical to `models/decision/rl_agent_config.json` |
| `tokenizer/tokenizer.json` | 3,583,228 | `6c8aaa9a542084f2457eab775d4eeb51f92a70c0fd9de28d5edb0ddec3c08d30` — byte-identical to `models/decision/tokenizer.json` |

### laya-rs 0.1.0 (what was read, what we use)

Published 2026-09-20 (one version; `repository` = `github.com/bob-rietveld/laya-rs`, **404**). Source read from the crates.io tarball (`/tmp/laya-design/laya-rs/`). Lib name `laya`; depends on `laya-core` 0.1.0 (same author: tokenizer wrapper over `tokenizers` 0.23 + onig, question parsing, calibration), `candle-core`/`candle-nn` **0.11**, `rayon`, `serde_json` (`preserve_order`), `thiserror`; optional `hf-hub` (`hub`) and `clap` (`cli`) — both off here. Features `metal`, `cuda`, `accelerate`, `mkl` are pure forwards to candle. `rust-version` 1.82.

- `model.rs`: `ModernBert` (embeddings, 28 `EncoderLayer`s with per-layer global/local RoPE tables built in f32 then cast, additive `(B,1,1,S)` / `(B,1,S,S)` masks built once per forward — the sliding mask is bounded from both sides because candle Metal has no `uabs` for I64, and padded *queries* may attend so no row is all `-inf`; exact-erf GELU) + `DecisionModel` (head layers with ReLU FFN, `type_emb`, scorer `LayerNorm→Linear→GELU→Linear`, act head `Linear(dims+4,256)→GELU→Linear`). **`forward` returns raw scorer logits `(B, slots)` f32 with unused slots at −1e4 and raw act logits `(B, 2)` f32** — no softmax, no temperature — so they go straight into our `postprocess`; nothing is calibrated twice. The act head's four extra features (top1, top1−top2, normalised entropy, K/255) are computed inside the model from a plain softmax over the masked slots, exactly as upstream.
- `weights.rs`: `SanitizedBackend` = a `SimpleBackend` view over `MmapedSafetensors` that renames `in_proj_weight/bias → in_proj.weight/bias` and inserts `.layers.` under `scorer`/`act_head` on lookup (PyTorch layout → their module names) without copying.
- `config.rs`: `EncoderConfig::from_value` (serde over `encoder/config.json`, validated like upstream's `from_dict`), `AgentConfig::from_value(raw, max_position_embeddings)` (`head_layers`, `max_len`, `head_max_len`, `act_classes = len(act_costs)+1`, temperatures). Their `max_len` defaults to the encoder's 8192 when the JSON omits it — ours says 512, which wins.
- `agent.rs` (not used — our `build_sequence`/`collate`/`postprocess` replace it) shows the input dtypes the forward is tested with: `input_ids u32`, `attention_mask u8`, `marker_pos i64`, `marker_mask u8`, `qtype u32`, and **slots floored at 2** (`sorted.i((.., slots-2))` in the act features would panic on K = 1). `tests/model_parity.rs`: NumPy reference on a synthetic tiny checkpoint, tolerance 2e-4 relative.
- Looks wrong / unmaintained: nothing wrong in the parts we exercise (parity below is exact on f32). The repo being 404 one day after publication means no issue tracker, no CI, no way to know if 0.1.0 is the last release — **vendor the 3.3 k lines (`src/{model,weights,config}.rs` are the ~1.2 k we need) before making it the default engine**, and drop `laya-core` (we only use it transitively for nothing). `Cargo.lock` gained 25 packages (candle-core/nn/metal-kernels/ug, gemm-*, pulp, float8, metal, ug, yoke, laya-core, laya-rs, a second `safetensors` 0.8 beside 0.4.5, a second `tokenizers` 0.22.2 beside 0.23.2 — candle-core 0.11 pins `tokenizers ^0.22` as a normal dep). No conflicts. **First build**: 1 m 37 s wall for `cargo build --release -p decision --features candle --lib` from a clean candle graph (865 s user); the `accelerate` variant 1 m 52 s.

### What was built (`b31bae3`)

- Workspace: `laya-rs = "=0.1.0"` (no default features), `candle-core`/`candle-nn` 0.11. `decision` features **`candle`** (`dep:` all three; `[target.'cfg(target_os = "macos")'.dependencies] laya-rs = { features = ["metal"] }` so `--features candle` is Metal on a Mac and CPU elsewhere) and **`accelerate`** (forwards `laya-rs/accelerate`). The `theuth` binary does not enable `candle` yet (default unchanged per the brief).
- `crates/decision/src/candle.rs` (`#[cfg(feature = "candle")]`): `CandleLoaded::open(dir, metal: bool, dtype)` — `Device::new_metal(0)` or `Cpu` (a Metal failure warns and falls back to CPU f32), our `LayaConfig` **and** their `EncoderConfig`/`AgentConfig` from the same two JSONs, `tokenizer/tokenizer.json` through our `tokenizers`, streamed sha256 of `model.safetensors` (`hash_time` recorded separately), `MmapedSafetensors → SanitizedBackend → VarBuilder(dtype, device) → DecisionModel::load`. `inputs(&Batch, &Device) -> [Tensor; 5]` converts our `collate` output to their dtypes. `run(&[Sequence])`: `collate(rows, Some((0, 2)))` (dynamic S and K, K ≥ 2, no bucketing), forward, `flatten_all().to_vec1::<f32>()` (the Metal sync), a non-finite check that names device/dtype/S/K, then our `postprocess` per row. `impl Oracle for CandleLoaded`.
- `engine.rs`: `MODEL_FILES` (ORT) beside `CANDLE_FILES` (`model.safetensors`, `encoder/config.json`, `tokenizer/tokenizer.json`, `rl_agent_config.json`), `model_files(EngineKind)`, `missing_files()` by engine (`ModelMissing` semantics intact); the production `Loader` is now a plain `fn open(dir, data_dir, cfg)` that matches `cfg.engine` — ORT as before, candle → `CandleLoaded::open(dir, cfg.device.metal(), F16 if Metal else F32)`, and a clear error when the build has no `candle` feature. **Deviation**: no `Provider::Candle` variant — `Provider` stays the ORT session's EP enum (`Loaded::open` could never take a candle variant); the engine choice is `DecisionConfig.engine: EngineKind { Ort (default), Candle }` plus `DecisionConfig.device: Device { Auto (default), Cpu, Metal }` in `lib.rs`, `auto` = Metal on `aarch64-apple-darwin`. Default model dir by engine: `models/decision` (ORT) or **`models/decision-candle`** (`CANDLE_SUBDIR`). The generated config comment in `theuth/src/main.rs` documents `engine`, `device`, both layouts.
- Tests: `candle::tests::inputs_match_the_forward_signature` (shapes `[B,S] [B,S] [B,K] [B,K] [B]`, dtypes `U32 U8 I64 U8 U32`, K floored at 2 for a one-marker batch, padding rows as expected); `tests/candle.rs` (model-gated on `~/.theuth/models/decision-candle` or `$DECISION_CANDLE_DIR`): the README four questions on CPU f32 and on Metal f16 against **`tests/fixtures/probs-cpu-fp32.json`** (the step-2d ORT fp32 CPU dump copied into the repo), argmax equal and max |Δp| ≤ 0.01, plus single-row = batched-row.

### Bench (`cae1745`; M3 Max, 20 warm runs per row, S/K dynamic, no bucketing)

`bench --engine candle --device metal|cpu --dtype f16|f32 [--dump F] [--ref F]`; outputs `/tmp/laya-design/bench-2e-{metal-f16,metal-f32,cpu-f32,cpu-f32-accelerate}.txt` (plus `metal-f16-run2/run4` repeats). RSS is reported three ways because they disagree: `ru_maxrss` (peak, counts the clean mmap'd file pages while mapped), `ps -o rss=` after the first inference (current), and `footprint` (`phys_footprint`, the unified-memory number Activity Monitor shows; sampled once during the warm rows of an f16 run: **1094 MB steady, 1102 MB process peak — 820 MB of it `IOAccelerator (graphics)`, i.e. the fp16 weights on the GPU, 14 MB mapped file, ~45 MB malloc**).

| engine / dtype | cold load (of which sha256) | first inference | RSS peak / now after first inference | K2 S339 p50/p95 ms | K2 S512 | K5 S393 | K5 S512 | K10 S473 | K10 S512 | README 4-Q batch | parity vs ORT fp32 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **candle Metal f16** (the candidate) | 2.05 s (1.87 s) | 93 ms | 1680 MiB / **903 MiB** (footprint 1094 MB) | **73.9 / 74.3** | 110.5 / 110.9 | 88.0 / 88.3 | 110.4 / 111.0 | 105.9 / 106.5 | 110.2 / 111.0 | **72 ms** | 4/4 argmax, max \|Δp\| **0.0006** |
| candle Metal f32 | 2.06–2.11 s (1.84–1.90 s) | 425 ms / 88 ms | 1678 MiB / 393 MiB (the 1.69 GB of f32 weights live in Metal buffers `ps` does not count) | 76.6 / 77.1 (2nd run 103.8 / 140.2) | 116.4 / 117.0 | 91.5 / 92.3 | 116.8 / 117.4 | 112.4 / 113.4 | 117.1 / 117.8 | 166 ms | 4/4, **0.0000** |
| candle CPU f32 (plain candle gemm) | 2.17 s (1.86 s) | 1375 ms | 2579 MiB / 1848 MiB | 1410 / 1496 | 1959 / 2677 | 1318 / 1577 | 1532 / 1691 | 1264 / 1426 | 1283 / 1311 | 1297 ms | 4/4, 0.0000 |
| candle CPU f32 + `accelerate` (builds cleanly, separate target dir) | 2.18 s (1.85 s) | 335 ms | 2580 MiB / 1822 MiB | 422 / 763 | 790 / 986 | 593 / 704 | 759 / 906 | 875 / 1958 | 759 / 861 | 442 ms | 4/4, 0.0000 |
| *ORT CPU fp32, step 2d, for scale* | 4.8 s | 585 ms | 2313 MiB | 581 / 585 | 590 / 605 | 627 / 647 | 662 / 685 | 679 / 692 | 679 / 695 | 2757 ms | reference |

Reading the table:

- **Metal f16 ≈ Metal f32 in latency, and 4 questions ≈ 1 question** (72 ms for the README batch vs 74 ms for one K2 S339 row; the batch rows are short, S ≈ 150). The GPU is not the bottleneck; candle's per-op Metal dispatch is — ModernBERT-large is 28 layers × ~20 unfused ops, each a kernel launch, plus the `(B,1,S,S)` mask build and a final `to_vec1` sync. Latency grows with S (74 → 110 ms from S 339 to 512), barely with K. Run-to-run: K2 S339 p50 73.8 / 73.9 / 85.3 (the last with `footprint` sampling in parallel) on f16, 76.6 / 103.8 on f32 — the machine was otherwise idle; GPU clock state is the likely variable. Batching is nearly free on Metal, the opposite of ORT CPU (4.7× for 4 questions).
- **The first-ever process on a machine pays a Metal shader compile**: run 1 of f16 measured 3.7 s inside `open` (the RoPE `cos`/`sin` and cast kernels) and **5.6 s first inference**; from run 2 on (macOS's Metal shader cache) `open` without the hash is ~0.16 s and the first inference 88–93 ms. candle compiles its Metal library from source per process and relies on the OS cache; nothing to persist on our side.
- **The cold load is our sha256**: 1.84–1.90 s of the 2.0–2.1 s is `io::copy` into `sha2` over 843 MB (≈ 450 MB/s — `sha2` without the `asm`/hardware feature on this toolchain); candle's own mmap + upload is ~0.16 s (f16) / ~0.2 s (f32, cast on the way). Same cost hides inside the ORT path's 2.2–4.8 s.
- **Memory**: `ru_maxrss` 1.68 GiB is a load-time transient — mmap'd fp16 pages resident while `VarBuilder` copies them into Metal buffers — and the mapping is gone after `open`; after the first inference the process is 903 MiB RSS and **1.09 GB footprint** (weights once, on the GPU, plus ~50 MB). On CPU f32 the weights are cast to f32 in RAM: 1.85 GiB now, 2.58 GiB peak (candle CPU keeps a copy per tensor; roughly ORT fp32's 2.3 GiB).
- **Correctness**: Metal f16 4/4 argmax and max |Δp| 0.0006 vs the ORT fp32 CPU probabilities; Metal f32 and both CPU f32 variants match to 4 decimals (0.0000). No NaNs at any S/K tried; candle's Metal kernels are fine on this graph, so there is no "Metal correctness" caveat to attach to the latency number.
- CPU: plain candle (its own `gemm`) is 2–3× slower than ORT fp32 CPU (1.3–2.0 s vs 0.58–0.72 s per question); with Apple `accelerate` it is at ORT parity or better on short rows (422 ms K2 S339, 590–880 ms otherwise) with a noisier p95. For 58 (Linux/Windows): candle CPU without a BLAS is not competitive; `mkl` (Intel) or CUDA is what those platforms would run, or ORT fp32 stays their CPU path.

### §12.8 verdict, criterion by criterion (Metal fp16, the candidate)

| criterion | required | measured | verdict |
|---|---|---|---|
| p95 warm, single question, Metal, S 339–512 / K 2–10 | ≤ 50 ms | 74.3 (K2 S339), 88.3 (K5 S393), 106.5 (K10 S473), 110.9–111.0 (S 512) ms — every row over; best row 1.5×, worst 2.2× the bar | **FAIL** |
| RSS after first inference | ≤ 1.5 GiB | steady **903 MiB** RSS / **1094 MB footprint** (one fp16 copy on the GPU); `ru_maxrss` peak 1680 MiB is the load-time transient (mmap pages + Metal upload overlapping) | **PASS** on the steady number the criterion names; the peak is over by 144 MiB and is recorded |
| max \|Δp\| vs ORT fp32 CPU on the README examples, argmax parity | ≤ 0.01, all agree | **0.0006**, 4/4 (f32 on Metal: 0.0000) | **PASS** |
| cold load | ≤ 3 s | **2.05 s** including 1.87 s of our own sha256 (0.16 s of candle) after the machine's first run; the first-ever run on a machine pays ~3.5 s of Metal shader compile on top (once, OS-cached) | **PASS** (shader-cache-cold first run: 5.6 s, fails once per machine) |

Three of four pass; latency fails by 1.5–2×, with the cause identified as launch overhead, not compute (fp16 = f32, batch ≈ single).

### Sky's decision (§12.8 Outcome, main `aeda3ff`)

**Candle on Metal is the v1 engine regardless of the 50 ms criterion**: 74–111 ms single, 72 ms for the 4-question batch is accepted; the meeting batch lands in ~70–100 ms against 2.8 s on ORT CPU. Battery is not a goal, so **Path B (Core ML / ANE) is dropped**, not deferred. §12.6's two-artifact-per-platform scheme is void: one `decision/` checkpoint dir everywhere. Follow-ups, in order (none done in this task — defaults unchanged, ORT path intact, as briefed):

1. The sha256 leaves the hot load path (verify once at download, or hash in the background) → ~0.2 s load.
2. `decision.engine` default becomes `candle`, `device: auto` (Metal on Apple silicon, CUDA where built, CPU + Accelerate/MKL otherwise); the ORT path and the fp16/fp32 ONNX artifacts are removed from `decision` (ORT stays for parakeet); 2b publishes the safetensors checkpoint dir instead — `release-models.sh` REQUIRED becomes `decision/{model.safetensors,encoder/config.json,tokenizer/tokenizer.json,rl_agent_config.json}`.
3. An optimisation pass on the Metal path with the apiplant tricks (explicit zero bias so LayerNorm takes the fused kernel, flattened GEMM, fused RoPE/GeGLU), 50 ms as the target, not a gate. Vendoring laya-rs's `model.rs` is the prerequisite for (3) and the insurance against the 404 repo.

### Checks

`cargo check --workspace` green (`--all-targets` fails in `nodes` on a missing generated `bindings/methods.json` — pre-existing, unrelated); `cargo check -p decision --features candle --all-targets` green; `cargo test --release -p decision --features candle`: **33 unit + 2 candle model-gated (CPU f32 Δp 0.0000, Metal f16 Δp 0.0006) + 3 ORT model-gated**, all pass; `cargo clippy -p decision --no-deps --all-targets` clean with and without `--features candle` (one `large_enum_variant` in the bench fixed by boxing). `cargo test -p tools`' two pty failures in non-tty shells not re-run.

## As built — step 3b (2026-09-21): `agent.escalate_model` + `SideCall` in `agent`

Brief: `/tmp/laya-design/omp-step3b.md`. `crates/decision` untouched (being refactored in a worktree); everything lives in `agent`, `rpc` (the RPC surface is there, not in `theuth`), `desktop`, `theuth` (config comment). Commits `2aa09eb` (slot + RPC), `c0c39c1` (side call; title turn on it), `d2e761c` (desktop + config comment), this note. `cargo test -p agent`: 238 passed (4 new).

### The slot (`2aa09eb`)

- `AgentConfig.escalate_model: Option<String>`; `Agents.escalate_model: Arc<RwLock<Option<Resolved>>>` beside `title_model`/`vision_model`, handed to every `Session::new` (one more positional arg; all six call sites updated), stored on `Shared`.
- Startup: resolved with the title model's leniency — a bad id logs `agent.escalate_model: …; tier 2 inherits the title model` and the slot stays `None`.
- `SetEscalateModel { model }` → `Result<Option<ModelInfo>, String>`: `escalate_candidate` = `catalog.resolve` + **exact catalog entry required** (text capability is enough; the title setter itself never checks existence, so "same validation" here means the stricter of the two — a typo must not become a pass-through model that answers nothing), then write `config.yaml` → swap live (the vision setter's order: a failed write changes nothing). `None` clears (`escalate_model: ` written empty, like the others).
- `GetEscalateModel` → **`EscalateModel { model: Option<ModelInfo>, effective: Option<ModelInfo> }`**: `effective` = escalate, else title, else `None` = "each session's own model" (there is no single id to show). Exported to the TS bindings (`bindings/EscalateModel.ts`).
- RPC `escalate_model()` / `set_escalate_model(model)` in `crates/rpc/src/api.rs`, dispatching to `Agents` like `title_model`.
- Tests (`supervisor.rs`): `set_escalate_model_validates_and_reports_effective` (unknown id → `not a catalog entry`, ambiguous bare id → `ambiguous`, both leave the slot and the file untouched; bare unique `blind` canonicalises to `testvis/blind` with `image: false`; `effective` follows the title model when unset; a write failure leaves the live value; `None` clears), `escalate_model_config_round_trip` (serde_yaml in/out, absent = `None`).

### The side call (`c0c39c1`) — what `decision::escalate` calls in step 4

```rust
// crates/agent/src/lib.rs
pub enum ModelRole { Escalate, Title, Vision, Session, Named(String) }
pub struct SideCall {
    pub session: SessionId,      // cost owner; also what `ModelRole::Session` means
    pub role: ModelRole,         // Escalate = escalate_model → title_model → the session's model
    pub system: String,
    pub user: String,            // the one user message (no history, no tools)
    pub budget: Duration,        // whole-call wall clock → Err("timed out")
    pub kind: CallKind,          // model_calls.kind: CallKind::Escalate ("escalate") for the oracle
}
pub struct SideReply { pub text: String, pub model: String, pub input_tokens: Option<u32>, pub output_tokens: Option<u32>, pub latency: Duration }

// from any plugin (the route `meeting::session::deliver` already uses):
let agents = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)?.ok_or("agent is not running")?;
let reply: Result<agent::SideReply, String> = agents
    .ask(agent::SideCall { session, role: agent::ModelRole::Escalate, system, user, budget, kind: agent::CallKind::Escalate })
    .await
    .map_err(|e| e.to_string())?;   // SendError<_, String> displays the handler's own message
```

- **Why the kameo registry and not a new function on the agent plugin API**: `decision` cannot depend on `agent` for a *type* it links against… it can — `agent` is below nothing that `decision` needs, but 00 §10's order is `runtime ← tools ← decision ← {meeting, browser}` and `agent` sits after `decision`; a direct function call would invert the edge. Kameo's name registry is the one cross-plugin channel that already exists (`meeting` steers sessions through `ActorRef::<Agents>::lookup(AGENT_ACTOR)`), needs no new runtime type, and `decision` only needs the message structs, which are plain data. Note for step 4: `decision` referencing `agent::SideCall` *is* a crate edge `decision → agent`; if 00 §10 is to hold literally, the message types move to `runtime` (they depend only on `SessionId`, `Duration`, `CallKind` — `CallKind` would have to move or be mirrored). Sky's call; the code is a two-file move either way.
- **Where it runs**: `Session` handles `SideCall` (`DelegatedReply`): it resolves the role from its slots and its own `Current` (the only place the session's model and its resolution error are known), snapshots `Shared::side(turn)` and spawns the call — the actor's mailbox and any running turn never wait on the provider. `Agents` handles `SideCall` by forwarding to the session's native actor the same way (`DelegatedReply`, spawned `ask`); an ACP session answers `session runs on an external harness`, an unknown one `no live session <id>`.
- **`loop_/side.rs`**: `Shared::role_model` — `Escalate` = escalate → title → session; `Title` = title → session; `Vision` = the fallback or `no vision fallback model is set` (never the main model, 52 §3); `Session` = the session's model or its `model: <error>`; `Named(id)` = `catalog.resolve` with the auth-derived `configured`. `Side::call` = `Current::new` → transport → `Frame { tools: &[], system, history: [user] }` → `Reserved::open(kind)` → stream drained to `End` under `tokio::select!` on the session's `closed` token and `timeout(budget)` → `settle(Settled | Failed | Cancelled)` → `SideReply`. Latency is dispatch→End. Genai has no non-streaming call here; "non-streaming" means no live events are published and the text is returned whole.
- **Title turn** (`maybe_title`): `role_model(Title)` + `side.call(model, title::SYSTEM, digest, 45 s, CallKind::Title)`; the digest, the revision check, the `closed` fences and the `TitleChanged` publish are unchanged. One behaviour change: the transport for a configured title model is now resolved inside the detached task, so a transport failure no longer returns *before* the title bookkeeping (`title_turns` reset etc.) — it is logged as `title: <error>` like any other failure. No existing test drove the old path end to end; `title::due` tests unchanged.
- Tests (`loop_.rs`, fake OpenAI gateway): `role_model_inherits_escalate_title_session` (the full chain, `Vision` never falls back, `Session` reports the main model's error while `Escalate` still resolves); `side_call_settles_under_its_kind` (one `escalate`/`settled` row with the provider's `cost` 0.25 → `returned`, `input/output_tokens` 10/5, `ModelUsageChanged` published; a stream that never reaches `End` settles `escalate`/`failed` and returns the error).

### Desktop (`d2e761c`)

- Settings → Agent loop → Helper models: **Escalation model** `Select` after the vision picker; `Off` renders as `Off (inherits title model <id>)` when a title model is set, description names tier 2 and the inheritance rule, `aria-label="Escalation model"`. Setting the title model also invalidates the escalation query so the inherited id follows.
- Slash: **`/escalate <model|off>`** — the repo's convention is `/title`, `/vision`, `/subagent` (not `/title-model`), so the brief's `/escalate-model` became `/escalate`. Bare `/escalate` opens Settings → Agent loop like its siblings. The TUI command table (`tui/model.rs`) was **not** extended — out of the brief's crates; `/escalate` there is a follow-up if wanted.
- `api.ts`: `escalate_model(): Promise<EscalateModel>`, `set_escalate_model(model)`, both in the method-name list; `queryKeys.escalateModel`; the reload hook invalidates it with the other model keys.
- Generated config: `# escalate_model: …` two lines after `vision_model`.

### Checks

- `cargo check --workspace --all-targets` green after each commit; `cargo test -p agent`: 238 passed (234 → 238); `cargo clippy -p agent -p rpc -p desktop --no-deps --all-targets`: no finding in changed code — the 14 findings it reports are all pre-existing (`network.rs`, `store/communication.rs`, `repl.rs`, `accounting.rs`, `loop_.rs:2565 stream_once` too-many-arguments, `supervisor.rs:3306`, `vision_fallback_tests.rs:556`), untouched here.
- `just ts-desktop` regenerated `bindings/` (+`EscalateModel.ts`); `bun run check` (`tsc -b --noEmit`) reports one pre-existing error outside the UI tree (`packages/apps/src/renderer.tsx: Cannot find module '@openuidev/react-lang/theuth-runtime'`), nothing in `crates/desktop/ui/src`.
- Not run: the desktop app itself (no `cargo tauri dev` here); the picker is the vision picker's JSX with two strings changed. A real `SideCall` against a provider was not exercised beyond the fake gateway — step 3b's "one real call against `agent.title_model`" check needs a configured provider on this machine and is left to step 4's integration test.
- `cargo fmt` reflows untouched code across `agent` (the crate is not fmt-clean); only the changed hunks were kept formatted, the rest reverted, as in earlier steps.

## As built — follow-ups (1)+(2) (2026-09-21): candle is the engine; laya-rs vendored; sha256 off the load path

Brief: `/tmp/laya-design/omp-followup-12.md`, executing the §12.8 Outcome's follow-ups (1) and (2). Commits on `feat/decision-default` off main `79dd8f6`: `abaad81` (vendor + NOTICE), `9b645ae` (candle is the engine — breaking), `3f76b52` (hash off the load path), this note. Machine state during this task: another agent's `cargo test -p agent`, `cargo check -p rpc` and two BoringSSL cmake builds ran in `../theuth` throughout — **load average 15–100 during every measurement below**, recorded per table. Sky's instruction: bench anyway, record the load, re-run K2/S339 once at the end.

### Vendoring (`abaad81`)

`crates/decision/src/candle/{model,weights,config}.rs` are laya-rs 0.1.0's `src/{model,weights,config}.rs` from the crates.io tarball, 1,210 lines; `candle/mod.rs` is the former `candle.rs`. `crates/decision/NOTICE` records license (Apache-2.0 per the crate's README — the tarball ships no LICENSE/NOTICE file, its NOTICE link points at the 404 repo), source URL, publisher and every local change:

- every file: a provenance header + `#![allow(missing_docs, clippy::manual_is_multiple_of)]` (our lints: `missing_docs = warn`, `clippy::all = deny`, and this toolchain's `manual_is_multiple_of` post-dates the code);
- `config.rs`: `laya_core::error::{Error, Result}` → a local `Error::Config(String)` + `Result<T, E = Error>`; `AgentConfig::temperature_for` (laya-core's `temp_bucket`; ours is `LayaConfig::temperature`) and the `raw: Value` field (their `convert` command) removed;
- `model.rs`: `crate::config` → `super::config`; the Metal mask regression test gated on `cfg(target_os = "macos")` — there is no crate feature `metal` here, Metal is a target dependency.

Their 14 unit tests (config validation, name sanitising, the sliding mask incl. the Metal-vs-CPU bit-for-bit check) run in ours. Dependencies: `laya-rs` and `laya-core` gone; `candle-core`/`candle-nn` 0.11 are direct workspace deps; `decision`'s macOS target table turns on `metal` + `accelerate` (the CPU fallback on a Mac is Accelerate-BLAS now); feature `cuda` forwards to candle, off by default, for 58. **Still duplicated in `Cargo.lock`, and why**: `safetensors` 0.8.0 (candle-core's pin) beside 0.7.0 (`ptts` → `voice`) and 0.4.5 (another target's tree); `tokenizers` 0.22.2 — candle-core 0.11 lists `tokenizers ^0.22` as a *normal* dependency — beside our 0.23.2. Neither goes away without patching candle-core; no conflicts.

### Candle is the engine (`9b645ae`, breaking)

- `laya.rs` keeps the pure functions: `build_sequence`, `collate(rows, floor)` (the "bucket" is a floor now — `Loaded::run` passes `(0, 2)`: dynamic S, K ≥ 2 because the act head indexes `slots − 2`), `postprocess`, `LayaConfig`, `RowOut`, `Batch`. Gone: the ort `Session`, `Provider`/`Units`, `BUCKET`, CoreML EP registration + cache dir, `parse_partitions`, `verify_graph`, `floats`, `f16_to_f32`, and with them the `ort`, `half` and `models` dependencies (nothing else in the crate touched them; `ort` stays in the workspace for parakeet/voice/meeting). 1,130 → 693 lines incl. tests.
- `candle::Loaded` (was `CandleLoaded`) is the one `Oracle`. **`verify_header`** replaces `verify_graph`: before any weight is read, the safetensors header — `(name, dtype, shape)` per tensor — is checked against `encoder/config.json` + `rl_agent_config.json`: `tok_embeddings` is `[vocab_size, hidden_size]`, the last encoder layer (`encoder.layers.{L−1}.attn.Wqkv.weight`), the last head layer (`head.layers.{H−1}.self_attn.in_proj.weight`), `scorer.layers.3.weight`, `act_head.layers.2.weight`, `type_emb.weight` exist (names compared after `sanitize_name`, so the PyTorch export and an MLX export both pass), and every tensor is f16 / bf16 / f32. Failures are named (`missing tensors …`, `tok_embeddings is [..], encoder/config.json says [..]`, `` `x` is U8, expected … ``). Unit test `header_verification` covers both layouts, a layer short, a different vocabulary and a quantised export. The real file: 206 tensors, all F16 except `temperature F32 [3]`.
- Config: `decision.engine`, `threads`, `coreml` removed (serde ignores unknown keys, so old files still load); `device: auto | cpu | metal` stays (`auto` = Metal f16 on `aarch64-apple-darwin`, else CPU f32; `metal` warns and falls back to CPU f32 when no device). **`MODEL_SUBDIR = "decision"`; `MODEL_FILES = ["model.safetensors", "encoder/config.json", "tokenizer/tokenizer.json", "rl_agent_config.json"]`** — the Hugging Face `convaiinnovations/laya` root layout; `Engine::missing_files` names exactly these (unit test `missing_model_escalates_and_is_rechecked` asserts the four, then three after one lands). `Engine::new(cfg, model_dir, log)` / `with_loader` / `Loader` lose the `data_dir` only `ort_lib` needed.
- On this Mac: `~/.theuth/models/decision/` (fp16 ONNX + flat tokenizer) → **`~/.theuth/models/decision-onnx-ref/`** (kept, reference only; `decision-fp32/` untouched, same status); `~/.theuth/models/decision-candle/` → **`~/.theuth/models/decision/`**.
- `scripts/release-models.sh` `REQUIRED`: the three ONNX-layout entries → the four safetensors-layout paths (the script is path-agnostic; `pocket-tts/voices/…` was already nested). `crates/desktop/src/models.rs` needed nothing: `download` does `create_dir_all(dest.parent())`, the trust check allows any run of `Normal` components, and its own test uses `m/f.bin`. Config comment in `theuth/src/main.rs` rewritten (`device`, one `model_dir` layout, no `engine`/`threads`/`coreml`).
- Bench: `--device metal|cpu --dtype f16|f32 --dump --ref`; ORT flags and the two-engine enum gone. `tests/model.rs`: the README four questions on `Device::Auto` against `tests/fixtures/probs-cpu-fp32.json` (argmax + |Δp| ≤ 0.01; the fixture is the step-2d ORT fp32 CPU dump and stays the reference precisely because it came from an independent runtime and export — noted in the test and bench docs) + the actor end to end; `tests/candle.rs` folded in.

### sha256 off the load path (`3f76b52`)

Integrity is verified once, at download (`desktop/src/models.rs` streams into `Sha256` and refuses a size or digest mismatch against `models.json`). At load, the safetensors header parse *is* the size check (offsets past the file are refused) and `verify_header` the structure check. The hash is only the log's identity, so:

- `Oracle::model_sha()` returns a shared **`ModelSha = Arc<Mutex<String>>`**, seeded by `Loaded::open` with **`size:<bytes>`** (`size:842609210` for this checkpoint) and overwritten by a background `std::thread` that starts **after** `DecisionModel::load` — started before the upload it competed with the mmap for the page cache (681 ms load in that variant). The engine reads it per row (`escalate`, `decide`, the blocking run) instead of copying once at load; the "model loaded" line prints whatever it is at that moment.
- **Rows written before the hash lands carry `model_sha: "size:842609210"` and a `gate_version` derived from that string** (the brief's "size-based tag" option, not `null`: the column stays a non-empty string and the fold does not care). The window is the hash's ~2 s once per *download*, not per load: the thread caches `<hex> <size> <mtime>` in **`model.safetensors.sha256`** beside the weights and reuses it while size and mtime match, so an `idle_unload` reload every quiet ten minutes does not re-read 843 MB. A sidecar that cannot be written is a debug line.
- `AskRow.model_sha` doc updated; `hash_time` is gone from `Loaded`.

### Bench (`examples/bench.rs`, M3 Max, 20 warm runs per row, S/K dynamic, **load average 15–27 from other sessions' builds**)

Outputs `/tmp/laya-design/bench-fu12-{metal-f16,cpu-f32,metal-f16-run1,metal-f16-run2,metal-f16-final}.txt`, each with `uptime` before and after.

| device / dtype | load avg | cold load | first inference | RSS peak / now after first inference | K2 S339 p50/p95 ms | K2 S512 | K5 S393 | K5 S512 | K10 S473 | K10 S512 | README 4-Q | parity vs fp32 ref |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Metal f16 (sidecar present) | 16.3 | **1177 ms** | 96 ms | 1674 / **897 MiB** | 119.1 / 163.7 | 176.8 / 184.8 | 121.1 / 131.4 | 169.8 / 184.3 | 159.3 / 204.2 | 164.2 / 182.0 | 96 ms | 4/4, **0.0006** |
| Metal f16, final K2/S339 re-run | 26.9 | 1158 ms | 97 ms | 1674 / 897 MiB | 126.4 / 203.1 | | | | | | 114 ms | |
| Metal f16, first load after the cutover (sidecar absent → hash thread runs during the rows) | ~30 | 1275 ms | 100 ms | 1674 / 900 MiB | 156.9 / 201.4 | 265.6 / 313.7 | 224.9 / 256.8 | 207.5 / 303.3 | 307.9 / 328.0 | 315.3 / 340.9 | — | — |
| Metal f16, second load (sidecar hit) | ~30 | **316 ms** | 105 ms | 1674 / 898 MiB | 138.9 / 178.9 | 243.3 / 288.6 | 165.0 / 189.6 | 242.5 / 282.3 | 203.6 / 280.8 | 204.2 / 222.4 | — | — |
| CPU f32 + Accelerate (now the default CPU build on a Mac) | 15.2 → 100 | **328 ms** | 342 ms | 2574 / 1820 MiB | 320.3 / 325.9 | 477.8 / 521.6 | 524.5 / 1024.2 | 731.7 / 1223.1 | 593.8 / 953.5 | 543.6 / 836.0 | 355 ms | 4/4, 0.0000 |
| *step 2e, quiet machine, hash inside load, for scale* | ~1 | 2.05 s (1.87 s sha256) | 93 ms | 1680 / 903 MiB | 73.9 / 74.3 | 110.5 / 110.9 | 88.0 / 88.3 | 110.4 / 111.0 | 105.9 / 106.5 | 110.2 / 111.0 | 72 ms | 4/4, 0.0006 |

Reading it:

- **The hash is gone from the load**: `model id at load` is `size:842609210` in every run and the hex by the end of the bench (`model id now`). What remains of the cold load is mmap + header check + Metal upload (or the f16→f32 cast on CPU), and *that* is what the load average inflates: **316–328 ms** in the two runs that met the ≤ 0.5 s goal (one Metal, one CPU), 1.16–1.28 s in the three Metal runs that did not, versus ~0.16–0.2 s for the same work on the quiet machine in step 2e. The goal is met by construction and on the CPU path under load; the Metal upload under a load average of 16–27 is not a number this session can clean — re-measure on a quiet box before quoting it.
- **Latency is 1.6–2.7× step 2e's** (K2 S339 p50 119–157 ms vs 74; p95 164–203 vs 74) with the same probabilities to 4 decimals, the same RSS and the same first-inference time. The per-op Metal dispatch is CPU-side work fighting 30–100 runnable threads; the final K2/S339 re-run (load 27) is in the same band, so this is the machine, not a regression — nothing in this task touches the forward pass. CPU f32 with Accelerate, oddly, came out *better* than step 2e's Accelerate run on the K2 rows (320 vs 422 ms p50) and worse on its p95s — same story.
- **Memory unchanged**: 897–900 MiB RSS after the first inference on Metal (one fp16 copy on the GPU + runtime), 1674 MiB `ru_maxrss` peak = the upload transient; CPU f32 1820 MiB now / 2574 MiB peak.
- Parity unchanged: Metal f16 4/4, max |Δp| 0.0006; CPU f32 4/4, 0.0000.

### The daemon

`cargo build --release -p theuth` with candle in: **12 m 42 s wall** (7,771 s user; load 40–70), `target/release/t` = **393,581,488 B**. Baseline at main `79dd8f6` (decision on ORT, no candle) built the same way in a throwaway worktree: **12 m 47 s** (7,948 s user), **390,310,432 B** → candle + Metal kernels + Accelerate binding cost **+3,271,056 B (+0.84 %)** and no build time (ORT is still compiled for `voice`). `t --data-dir /tmp/theuth-hl --headless` with `decision: {enabled: true}` (RPC on a free port — the app's launch environment exports `THEUTH_RPC__LISTEN`, which is why the first two attempts hit "address in use") logs `started plugin="decision"` and `rpc listening`, never loads the model (no `model loaded` line, no `decisions/` dir, RSS 100 MiB after 5 s): lazy as before.

### Checks

`cargo clippy -p decision --no-deps --all-targets` clean; `cargo test --release -p decision`: **44 unit** (30 ours + 14 vendored) **+ 2 model-gated** (README parity on `Device::Auto` = Metal f16, Δp 0.0006; actor end to end) pass; `cargo check --workspace --all-targets`: only the pre-existing `nodes` lib-test error (`include_str!("../../../bindings/methods.json")`; `bindings/` is gitignored and written by `cargo test -p rpc`'s export test, absent in a fresh worktree). `cargo test -p tools`' two pty failures not re-run.

### Open

- Follow-up (3) — the fused-kernel pass on `candle/model.rs` (zero-bias LayerNorm, flattened GEMM, fused RoPE/GeGLU) with 50 ms as the target — is now possible on vendored code; not started.
- 2b: publish `decision/{model.safetensors,encoder/config.json,tokenizer/tokenizer.json,rl_agent_config.json}` (842,609,210 + 2,083 + 3,583,228 + 745 B) to the bucket and drop the ONNX objects; `REQUIRED` already lists them.
- `decision.device: cuda` for 58 once the Linux build exists (`--features cuda` compiles the forward; `Device::new_cuda` is a two-line addition to `Device::metal()`'s match).
- The `theuth` binary is 375 MiB either way — worth a look at what else is in it, unrelated to decision.

## As built — step 4 (2026-09-21): meeting pre-filter, Off / Shadow / Active switch, ⚡ rows, island chip, `decision::escalate`

Brief: `/tmp/laya-design/omp-step4.md`. Commits on main: `48e5cbe` (side-call types → `runtime`), `2bcf73a` (`decision::escalate` + config/engine additions), `c9b867d` (meeting pre-filter + RPC), `a234ba7` (desktop), `d5d1ada` (lockfile), this note. `decision_stats` landed inside `c9b867d` rather than as its own commit (same file as the two mode RPCs).

### Commit 0 — the layering call (`48e5cbe`)

3b's open question is settled: `SideCall`, `SideReply`, `ModelRole`, `CallKind` live in **`runtime::side_call`** (`crates/runtime/src/side_call.rs`) and `agent` re-exports them (`pub use runtime::side_call::{ModelRole, SideCall, SideReply}` in `lib.rs`; `pub use runtime::side_call::CallKind` in `cost.rs`, replacing its `str_enum!` block — `as_str`/`Display`/`FromStr` are hand-written in `runtime` with the same strings, `ts(export)` kept behind `runtime/ts`). Nothing else changed: `agent::SideCall` etc. resolve as before; 00 §10's `runtime ← tools ← decision ← {meeting, browser} ← agent` holds — `decision` depends on `runtime` only. Note for 00: `meeting` already depends on `agent` (session steering), so the literal order there is `decision ← agent ← meeting`; `decision → agent` is the edge that matters and it does not exist.

### `decision::escalate` (`2bcf73a`) — deviations from the brief's signature

- **Signature**: `escalate(ask, oracle: Option<&Decision>, session, budget, call: &SideCaller)`. The brief's four-argument form cannot exist: kameo's registry lookup is typed (`ActorRef::<agent::Agents>::lookup`), and `decision` cannot name `agent::Agents`. `SideCaller` is `dyn Fn(SideCall) -> Pin<Box<dyn Future<Output = Result<SideReply, String>>>>`; `meeting::oracle::side_caller()` is the 3b snippet (lookup + `ask`), the unit tests pass a fake. No global.
- Prompt: `escalate::SYSTEM` (the JSON contract, `none` when nothing fits, never invent a label) + `render(ask, oracle)`: question, `Labels:` table (`- id: text`, `none` appended for a `Choice`; `true`/`false`; `- i: level`), the oracle's own estimate as one data line (`A local classifier estimated (abstained: low margin): SRE-2386 0.31, …`) when it ran, then `State:`. `parse` finds the first `{`, reads one JSON value (fences/prose around it tolerated, anything inside strict), requires `answer` ∈ the label set (bool for `Bool`, index or its string for `Score`), `confidence` a number clamped to 0..1 (missing = 1 — a missing number is not an unknown label). Unknown label → `EscalateError::Invalid`; the side call's `timed out` → `Timeout`; anything else → `Provider(msg)`.
- Result: `Decision { tier: 2, calibrated: false, probabilities = confidence on the pick, (1 − c)/(K − 1) on the rest, disposition = Accept, or Escalate(LowTop1) when the model itself answered none }`, logged as a `tier: 2` ask row through the engine's new `Record` message (`ActorRef::<Engine>::lookup(DECISION_ACTOR)`; silently skipped when the actor is not registered, i.e. in unit tests). `Decision` gained `tier: u8` (serde default 1) and `calibrated: bool` (default true); `AskRow` gained `tier`; `FamilyStats` gained `mode` (filled by the engine from the live config), `tier2`, `tier2_accepted`.
- Config: **`families:` is a patch over the default table, not a replacement** (the step-1 semantics "listing any family replaces this table" is gone — it made a one-key Settings write wipe four families). `meeting.topic: { mode: active }` changes one field; an unknown key with all of `mode`/`tau_top`/`tau_margin` is a new family; a **prefix key** (`meeting: { mode: off }`) patches every family under it and is applied after exact keys, so the switch wins over a hand edit. `FamilyGate` gained `budget` (humantime, default `400ms`) and `escalate_budget` (default `3s`). `mode_for(cfg, model_dir, prefix) → ModeInfo { mode, effective, reason, budget_ms, escalate_budget_ms }`: `mode` = the most permissive mode among the prefix's families (never under-reports an active one), `effective` = `off` with a reason when `decision.enabled: false` or a checkpoint file is missing. Engine messages: `GetMode { prefix }`, `SetMode { prefix, mode }` (writes `decision.families.<prefix>: { mode: … }` via `runtime::write_nested_config_value`, then live; a failed write changes nothing), `Record(Decision)`, `AllStats → Vec<FamilyStats>`. `decision` gained a `ts` feature (`Mode`, `ModeInfo`, `FamilyStats`).

### Meeting pre-filter (`c9b867d`) — `crates/meeting/src/oracle.rs`

- **Off the mailbox.** `Meeting::send_batch` (after the steer-queue check) spawns `oracle::run(rt, cache, blocked, batch, meeting, session)` and the task `tell`s a `Verdict { batch, action, event, expected }` back. Tier 1 is ~100–300 ms and tier 2 up to 3 s per question; the actor keeps taking `Piece`s and `Tick`s meanwhile. With the oracle off the only change to today's path is that one hop.
- **`run`**: `GetMode("meeting")` → `effective == off` → today's delivery, nothing emitted. Else `OracleDeciding { batch_id }` is published (`batch_id` = `hms(first utterance t0)`, the `[meeting hh:mm:ss]` stamp), then two asks on the decision actor: `Bool(TOPIC_STATEMENT, state = them:/me: lines)` (`meeting.topic`) and, when `Cache::search(text, 16)` minus the rows shown/dismissed this meeting leaves anything, `Choice(BRIEF_INSTRUCTIONS, top-8 rows as { id: row id, text: "title (aliases)" })` (`meeting.brief`). Two sequential asks, not one batched `AskMany` (the engine has no batch message; ~2 × 100 ms on Metal, under the 400 ms budget each). The mode the engine actually ran in is read off the topic decision (`Shadow(_)` → shadow; `Escalate(Disabled | ModelMissing)` → treated as off).
- **§3.2 in code** — `settle(topic, brief) → Settled::{Skip, Brief(id), Model(reason), Open{topic, brief, reason}}`, looking through shadow wrappers: a brief pick wins, then `topic=false`, then `topic=true` with no candidates or `brief=none` accepted = research (tier 3); anything unaccepted is `Open`. **Tier 2 only in active mode and only for the open questions** (brief first, then topic, each through `decision::escalate` with the family's `escalate_budget`); the answers are substituted and `settle` runs again; a failed/invalid/timed-out call leaves the question open and the reason reads `low margin; tier 2: timed out`. Shadow never spends a tier-2 call (its outcome must be the model's independent answer, and a side call per batch is what shadow is meant to avoid) — a deviation from "the cheap model gets the same two questions" for shadow only.
- **Actions** (`Message<Verdict>`): `Skip` → the batch is **carried** (`Live::carry`, capped at the last 6 batches — `ponytail:` in the code); `Publish { row, label }` → `Topics::insert_cached(label, row, BriefSource::Oracle, …)` + `TopicAdded` (same dedupe as the alias scan: a shown/dismissed row or a live/dismissed label just logs and skips — no turn spent either way), then carried; `Model { evidence }` → `session::batch_line_with(batch, live, dismissed, carried, evidence)`: the header gains ` · earlier (oracle skipped): 00:13:10, 00:13:40`, the carried utterances are written before this batch's rows (so the escalated turn still has them as context — the brief's "appended to the meeting frame silently" has no no-turn append in `agent`, this is the substitute), the evidence line `oracle: topic 0.62, brief none 0.55 / SRE-2386 0.31 (escalated: low margin)` last. The stop-time drain sends whatever is carried too. `[meeting request …]`, `[meeting tapped]`, `[meeting deeper]` never enter `send_batch`, so they bypass the oracle by construction.
- **Shadow outcome.** Every tier-3 delivery (shadow always; active when escalated) pushes `(event, Expected { (log_id, family, expected label)… })` on `Live::pending`; `AddTopic` (the model's `meeting_topic`) records `(cached row, title)` on `Live::published`; at the session's `TurnFinished` (not `error` — an errored turn is no label) `oracle::outcome` compares per family (topic: `true` iff anything was published; brief: the cached row id or `none`), `Report`s `Confirmed` / `Corrected { actual }` to the decision log against both log ids, and re-publishes `OracleDecision` with `outcome: Some(OracleOutcome { agreed, published })`. A steer that lands inside an already-running turn is judged with that turn's publications — the approximation is deliberate.
- **Event**: `MeetingEvent::OracleDeciding { meeting, batch_id }` and `OracleDecision { meeting, decision: OracleDecision { batch_id, family, mode, disposition: Skip | Brief { brief, title } | Model { reason }, top: [("topic", P(true)), (brief title, p)…, ("none", p)], latency_ms, tier, outcome } }` — one event per batch (not per family: the row is per batch; `family` names the one that settled it). `BriefSource::Oracle`; `Topic.source: Option<BriefSource>` (53's topic had no source enum — added, serde-default so old `briefs.jsonl` rows load). Plugin start logs `meeting: decision oracle <effective> (configured <mode>: <reason>)` from the config (the decision plugin starts after `meeting`).
- RPC (`crates/rpc/src/api.rs`): `meeting_oracle_mode() → ModeInfo`, `set_meeting_oracle_mode(mode) → ModeInfo`, `decision_stats() → Vec<FamilyStats>`; `rpc` and `meeting` now depend on `decision`.

### Desktop (`a234ba7`)

- **Settings → Meetings → "Local decision oracle"** (`components/settings/Meetings.tsx`, `OracleSwitch`): a `ToggleGroup` Off · Shadow · Active on `meeting_oracle_mode` / `set_meeting_oracle_mode` (query key `meetingOracleMode`), one help line per position (`ORACLE_MODE_HELP`, also the items' tooltips), disabled with `Off until this is fixed: <reason>` when `effective` is forced off.
- **Session ⚡ rows** (`components/chat/MeetingCard.tsx` `OracleRow`, text in `lib/meeting-line.ts::oracleRow`, stamps in `batchStamps`): a batch card looks up `store.oracle` by its own `hh:mm:ss` **and** the stamps its header names under `earlier (oracle skipped):` — so in active mode the skipped batches' rows appear under the card of the batch that finally reached the model, which is also where their text is. Row text as designed: `⚡ topic 0.12 · skip · 78 ms` / `⚡ brief SRE-2386 0.91 · published · 81 ms` / `⚡ escalate · low margin 0.55 / 0.31 · tier 3 → model`, `· tier 2` when the cheap model answered, then `· ✓ agreed` / `· ✗ model published HR-12` / `· ✗ model did nothing`; a `<details>` opens to every label's probability. Rows exist only for the live meeting (the store is not persisted) — a past meeting's session shows none.
- **Island chip** (`components/meeting/Island.tsx` `OracleChip`, right of the clock): mode badge from the last event's `mode` (falls back to `meeting_oracle_mode`, refetched every 30 s), hidden when off; live indicator `⚡ deciding` (`OracleDeciding` until its verdict) → `skip` (dim, fades after 4 s) / `brief` / `→ model`; tooltip = `store/meeting.ts::oracleSummary` (`⚡ 14 skipped · 3 briefs · 2 escalated · agreed 92 %`). Oracle-published topic buttons carry a `Zap` icon instead of the cached dot (`Topic.source === 'Oracle'`). The same summary sits in the Meetings tab header (`ControlCenter.tsx`, `MeetingHeader`) while live.
- Store: `oracle: OracleDecision[]` (last `ORACLE_KEEP = 50`, an outcome update replaces its batch's row in place), `deciding`, `oracleCounters` / `oracleSummary`; `Started` resets them. `api.ts` + method list, bindings regenerated (`just ts-desktop`: `OracleDecision`, `OracleDisposition`, `OracleOutcome`, `Mode`, `ModeInfo`, `FamilyStats`, `Topic.source`).

### Checks

- `cargo check --workspace --all-targets` green after each commit; `cargo clippy -p runtime -p decision -p meeting -p rpc --no-deps --all-targets` clean (one `large_enum_variant` on `oracle::Action` fixed by boxing the row); `agent` clippy findings unchanged (only re-exports touched).
- `cargo test -p runtime` 2 (1 new: `CallKind` string round trip); `-p decision` **49 unit** (44 → 49: config patch/prefix, `mode_for`, escalate render/parse/failure mapping) + model-gated; `-p meeting` **37 unit** (32 → 37: `settle` table incl. tier-2 resettle, evidence line + `top` labels, `outcome`, `state`/`batch_id`) + **`tests/oracle.rs`** model-gated: shadow → `Model { evidence: None }` + `OracleDeciding`/`OracleDecision` (mode shadow, `expected` set) — the SRE-2386 batch came out `topic 0.498 → low top-1`; active on small talk (`yeah, sounds good / ok, thanks, talk later`) → **`Skip`, tier 1, 188 ms, P(topic) 0.0018**; off → no event, today's action. `-p rpc` 17. Desktop `bun run check`: only the pre-existing `packages/apps/src/renderer.tsx` error; `bun run test` 193 (191 → 193: the store's oracle reducer/counters/ring, `oracleRow` + `batchStamps`).
- Smoke: `target/debug/t --data-dir /tmp/theuth-step4 --headless` with `decision: { enabled: true }` + `meeting: { enabled: true }` logs `meeting: decision oracle off (configured shadow: model missing: model.safetensors, … not in /tmp/theuth-step4/models/decision)` at plugin start; over the WebSocket `meeting_oracle_mode` → `{ mode: shadow, effective: off, reason: …, budget_ms: 400, escalate_budget_ms: 3000 }`, `set_meeting_oracle_mode("active")` → `mode: active` and `config.yaml` gains `decision:\n  families:\n    meeting: { mode: active }` (comments and the other keys intact), `"loud"` → `-32602 unknown variant`, `decision_stats` → five families with their modes (`meeting.*` active), `set…("shadow")` rewrites the same line. Restarted with `model_dir: ~/.theuth/models/decision` and the written `meeting: { mode: active }`: `meeting: decision oracle active`.
- Not run: the desktop app itself (`cargo tauri dev` not available here) — the three UI pieces are typechecked and unit-tested, not seen; a real meeting through the actor (needs a Core Audio tap + a provider) — the actor's `Verdict`/`TurnFinished` paths are exercised only by construction; a real tier-2 call against a provider (the fake caller covers the contract, `tests/oracle.rs` covers the fail-over to tier 3 without an agent).

### Open

- The integration test's first batch (`the deploy is blocked on SRE-2386, the image tag pipeline`) scores **P(topic) = 0.498** — the root checkpoint is unsure exactly on the kind of line the meeting family exists for. Shadow data will say whether `tau_top: 0.85` on `meeting.topic` ever accepts a `true`; the skip side (`0.0018` on small talk) is where the saving is anyway.
- `Ask` batching: the two questions are two mailbox round trips. An `AskMany` on the engine (one `collate` + one forward) halves the tier-1 latency; do it when the log shows the second ask timing out.
- The session's ⚡ rows live in the desktop store only; if Sky wants them on a past meeting's session, the verdicts need a `briefs.jsonl` record (the `Record` enum is the place).
- TUI `/decision` was not added (out of the brief's crates); `decision_stats` is the RPC it would read.

## As built — step 4b (2026-09-21): phrasing bench, mentions, `meeting.kind` Bool, asymmetric gate, tier-2 subject extraction, warm-up, staged ⚡ rows

Brief: `~/.theuth/omp-step4b.md` (§3.5 as the build order). Commits on main: `3217e3f` (phrasing bench + fixture), `09ee2d7` (mentions, `meeting.kind`, asymmetric gate, ≤ 4 candidates, warm-up), `9b07272` (tier-2 subject extraction, subject-led evidence, shadow runs the full pipeline), `55437f2` (wip checkpoint: `meeting-line.ts`/test + `store/meeting.ts` — the desktop half, finished by) `5e45674` (`store/meeting.test.ts`, tier-2 counter), this note. Toolchain: `export PATH="$(just toolchain):$PATH"`.

### Phrasing bench (`3217e3f`) — `crates/decision/examples/phrasing.rs`

Fixture `crates/decision/tests/fixtures/meeting-batches.json`: **112 batches** — today's recorded meetings ("Sre-standup-9-21-26" 15:00, "icdportal.com migration" 16:04) re-split offline with `batch.rs`'s flush rule as `them:`/`me:` lines, labelled from `briefs.jsonl`/outcomes, plus hand-written batches (small talk / scheduling, the Kuiper-belt narrative, technical talk without ticket vocabulary). Labels: 20 `small_talk`, 67 `named_subject`, 25 `general`; 19 carry a `published` brief, 11 of those have the brief among the FTS candidates. Three `published` labels on icd scheduling batches (Tony/Sky/cutover) are model noise and were left as labelled. Run: `cargo run --release -p decision --example phrasing` (model-gated on `~/.theuth/models/decision`), M3 Max, Metal f16, load average ~20–34 throughout (another agent's builds).

**1a — conversation kind.** `skips@τ` = small talk skipped / other batches skipped / skips that hid a published brief.

| # | variant | acc | mean P(correct) | confusion | skips@0.6 | skips@0.8 | p50/p95 ms |
|---|---|---|---|---|---|---|---|
| 1–4 | 3-way `Choice`, label set A (§3.5 text), 4 instructions | 25–27/112 | 0.265–0.285 | everything → general | 0–1 / 0 / 0 | 0 | ~50/70 |
| 5–8 | 3-way, label set B (short) | 24–27/112 | 0.265–0.268 | mostly general | 2–3 / 4–7 / 1–2 | 0 | ~55/75 |
| 9–12 | 3-way, label set C (concrete) | 28–33/112 | 0.296–0.311 | mostly general | 2 / 0 / 0 | 0 | ~53/66 |
| 13 | 2-way `Is this small talk or a conversation about something?` | 82/112 | 0.578 | `[[18,2],[28,64]]` | 13 / 7 / 2 | 0 | 53/64 |
| 14 | 2-way `Which describes the transcript?` | 48/112 | 0.500 | `[[20,0],[64,28]]` | 18 / 38 / 5 | 3 / 1 / 1 | 43/58 |
| 15 | 2-way `What kind of conversation is this?` | 44/112 | 0.481 | `[[20,0],[68,24]]` | 19 / 45 / 7 | 10 / 3 / 2 | 41/54 |
| **16** | **Bool `The speakers are only greeting, scheduling or saying goodbye`** | **101/112** | **0.799** | `[[90,2],[9,11]]` | **8 / 0 / 0** | 1 / 0 / 0 | 43/60 |
| 17 | Bool `This is small talk with nothing to look up` | 92/112 | 0.741 | `[[91,1],[19,1]]` | 0 / 0 / 0 | 0 | 44/60 |
| 18 | Bool `Nothing of substance is discussed: only greetings, thanks, scheduling or goodbyes` | 100/112 | 0.741 | `[[89,3],[9,11]]` | 8 / 1 / 0 | 2 / 0 / 0 | 48/63 |
| 19 | Bool `The speakers only exchange pleasantries, thanks, scheduling or goodbyes; no subject is discussed` | 100/112 | 0.756 | `[[92,0],[12,8]]` | 6 / 0 / 0 | 1 / 0 / 0 | 50/63 |
| 20 | Bool `The speakers discuss a specific named subject: …` (true = subject) | 21/112 | 0.281 | `[[20,0],[91,1]]` | 20 / 89 / 18 | 18 / 70 / 12 | 50/73 |
| 21 | Bool `The conversation has a subject someone could look up` | 22/112 | 0.293 | `[[20,0],[90,2]]` | 20 / 87 / 17 | 19 / 73 / 14 | 43/58 |

Reading: every 3-way `Choice` is near-uniform (mean P(correct) 0.27–0.31; everything lands on `general`) — the classifier cannot *name* the kind. The `Bool` head is biased to `false`, which makes it a usable **small-talk detector** (variant 16: 101/112, 8 of 20 small-talk batches skipped at τ 0.6, 0 false skips, 0 hidden briefs) and a useless **subject detector** (variants 20–21: P(true) low on everything, 18 hidden briefs at 0.6). τ_kind = **0.6** (the §3.5 default 0.8 skips 1 batch). Full per-variant rows (all 4 × 3 3-way variants) come out of the bench run.

**1b — cached brief.** Query = mentions ∪ batch text; K candidates by FTS rank + `none`.

| K | ids | acc | mean P(correct) | mean P(correct \| brief truth) | publish@0.6/0.3 right / wrong | none-accepted right / wrong | p50/p95 ms |
|---|---|---|---|---|---|---|---|
| 4 | uuid | 100/111 | 0.458 | 0.237 (n=11) | 0 / 0 | 15 / 0 | 89/107 |
| **4** | **b1..b4** | 76/111 | 0.421 | **0.481** | 3 / 6 | 24 / 0 | 59/76 |
| 8 | uuid | 80/111 | 0.243 | 0.130 | 0 / 0 | 1 / 0 | 102/121 |
| 8 | b1..b8 | 61/111 | 0.330 | 0.493 | 4 / 18 | 15 / 0 | 80/97 |

Reading: UUID ids never pick a brief (the 192-token head budget goes to the ids); short ids `b1..b4` double P(correct | a brief is the truth) and keep `none` clean; K8 publishes 18 wrong briefs. Most of the K4 "wrong" publishes are label leakage — the icd meeting's briefs were created *in* that meeting after the batch (icd-00:12:53 → SSL cert timeline 0.73, icd-00:09:32 → Domain redirect 0.75, icd-00:19:13 → Certificate & TLS 0.71 are right in substance); genuinely wrong at 0.6/0.3: sre-00:09:18 → Certificate & TLS 0.63 (Outlook/2FA talk), icd-00:18:54 → DR cutover 0.61 (ICD cutover window).

### Chosen phrasings and constants (`crates/meeting/src/oracle.rs`, `crates/decision/src/lib.rs::default_families`)

- `meeting.kind` = **`Bool("The speakers are only greeting, scheduling or saying goodbye")`** (`KIND_STATEMENT`), `true` = small talk; gate **`FamilyGate::shadow(0.60, 0.20)`**. Skip ⇔ P(true) ≥ 0.6 accepted **and** `mentions` empty.
- `meeting.brief` = **`Choice("which cached brief is the subject being discussed")`** (`BRIEF_INSTRUCTIONS`, unchanged) over the top-**4** FTS rows (`CANDIDATES`; query = `mentions.join(" ") + text`, `Cache::search(query, 12)` minus the rows shown/dismissed this meeting), candidate ids **`b1..b4`** (`candidate_id`), text = **title only**, + `none`; gate **`shadow(0.60, 0.30)`**. Publish ⇔ a brief accepted (P ≥ 0.6, margin ≥ 0.3).
- `meeting.subject` = the tier-2 family (`decision::subject::FAMILY`; rows only, never asked of the oracle), gate row `shadow(0.50, 0.0)` so `decision_stats` and the `meeting` prefix switch cover it.
- `meeting.topic` is **removed** from the default table (not disabled): the bench showed it confidently wrong, not uncertain. Sky's `config.yaml` `meeting: { mode: shadow }` prefix patch applies to the three new families unchanged (tested in `lib.rs`).
- `mentions(text)`: capitalised n-grams not at sentence start, ticket ids `[A-Z]{2,}-\d+`, acronyms (≥ 2 capitals), numbers with units, quoted terms; de-duplicated, occurrence order, ≤ 12 (`MAX_MENTIONS`). Kuiper batch → `["Mike", "Kuiper", "Neptune", "Earth", "Planet Nine"]`; small talk → empty.
- Tier 2 = `decision::subject` (`SYSTEM` prompt, `render`, `parse`, `extract`): the request is `Mentions:`, `Cached briefs:` as `- b1 — title (aliases)`, tier 1's numbers as one data line, then the transcript; the answer is `{ "subject": string|null, "cached": <offered id>|null, "lookup": bool }`, parsed strictly (`cached` must be an offered id or `none`/`null`; `lookup` missing = `subject` is set; subject cut at 80 chars). Dispositions in `oracle::run`: `cached` → Publish (same dedupe; tier2 text `cached: <title>`); `subject` + `lookup` → tier 3 with evidence **`oracle: subject "Planet Nine" — no cached brief; kind subject 1.00`** first in the batch delivery (`subject_evidence`); `subject` + `!lookup` or `subject: null` → Skip (`"…" → nothing to look up` / `nothing to look up`); invalid / timeout / provider error → tier 3 with today's evidence line and reason `<reason>; tier 2: <error>`. Budget = the family's `escalate_budget` (3 s).
- Warm-up: `MeetingState::Recording` (`lib.rs` at `Started`) spawns `oracle::warm_up()` → engine message `decision::Warm { budget: 30 s }`: load + one unlogged forward on a fixed `Bool("warm-up")`, logs `decision: warm-up done in N ms`.

### Deviations from §3.5 / the brief

- **`meeting.kind` is a `Bool`, not a 3-way `Choice`.** Every 3-way variant is near-uniform (table above); the Bool small-talk statement is the only phrasing with a usable skip side. Consequence: the oracle never says "named subject" — the ⚡ row's first stage reads `kind: small talk P` or `kind: subject P` (P of the winner), and "subject" there means "not only small talk".
- **τ_kind = 0.6**, not the brief's default 0.8 (0.8 skips one batch in 112).
- **Warm-up is an engine `Warm` message** (load + one unlogged forward), not a logged `Ask` on `meeting.kind` — a throwaway ask would land in the decision log and `decision_stats` as a real question.
- **Tier-2 rows carry the subject in `AskRow.note`** (new field, serde-default) under family `meeting.subject`, `choice` = the cached id / `lookup` / `skip`; `outcome` compares `SUBJECT` rows against the model's publication (cached row id, else `lookup` iff anything was published).
- **The evidence line moved to right after the batch header for every case**, not only the subject-led one (`session::batch_line_with`), so the research turn always reads the oracle's verdict before the transcript.
- **Shadow spends tier 2** (revert of step 4's "shadow never spends a tier-2 call"): Sky wants the whole pipeline evaluated; the shadow outcome is still the model's independent answer, judged per family at `TurnFinished`.
- `OracleDecision` on the wire: `stages: Vec<(String, f32)>` (`kind: small talk 0.79`, `brief: none 0.57`), `mentions`, `subject: Option<String>`, `tier2: Option<String>` (`"Planet Nine" → no cached brief` / `nothing to look up` / `timed out`), `top` = the brief candidates (titles) + `none` only (the topic entry is gone); `latency_ms` includes tier 2. `OracleDisposition::Brief.brief` is the cache row id, not `b1`; `Expected` rows likewise carry the row id so `outcome` compares against what the model publishes.
- Cached briefs on this machine are the SRE/ICD sets; the HV/MV cache from the first shadow session is gone (`01a0bfb*` ids are not in `briefs.sqlite`), so the fixture's brief candidates are the SRE/ICD titles.
- Desktop: the brief's `feat(desktop)` commit is split across `55437f2` (wip: `lib/meeting-line.ts::oracleRow` + test, `store/meeting.ts` counters) and `5e45674` (`store/meeting.test.ts`). Row text: `⚡ kind: subject 0.91 · mentions: Kuiper belt, Constantin · brief: none 0.57 · tier 2: "Planet Nine" → no cached brief → model · 1442 ms`, `⚡ kind: small talk 0.79 → skip · 78 ms`, `⚡ kind: subject 0.88 · brief: SRE-2386 0.91 → published SRE-2386 · 81 ms`; empty stages omitted; shadow appends `✓ agreed` / `✗ model published HR-12`. Counters/summary: `⚡ 1 skipped · 1 brief · 1 escalated · 0 tier 2 calls · agreed 50 %` (`tier2` = rows with `tier2 !== null`). Island chip text unchanged.

### Checks

- `cargo test -p decision -p meeting` (this note's run): `decision` **52 unit** (49 → 52: subject render/parse incl. cached-id validation and `subject: null`, `extract` contract with a fake `SideCaller`, default table without `meeting.topic`) + `tests/model.rs` 2 model-gated; `meeting` **38 unit** (37 → 38: `mentions` on the Kuiper batch and on small talk, the asymmetric `settle` table — small talk with mentions → tier 2, brief 0.65 / margin 0.2 → tier 2, kind timeout → tier 2, brief accepted wins; `stages`/`evidence`/`top`; `outcome` per family) + **`tests/oracle.rs` 1** model-gated, passing. `cargo clippy -p decision -p meeting --no-deps --all-targets` clean and `cargo check --workspace --all-targets` clean at `9b07272`; nothing in Rust changed since.
- Model-gated `tests/oracle.rs` (fake side caller answers `{"subject": "Planet Nine", "cached": null, "lookup": true}` for the Kuiper batch, `{"subject": null, "lookup": false}` for small talk), as printed this run (load average high, another build running): shadow Kuiper → **`Model { reason: "subject \"Planet Nine\", no cached brief" } tier 3 in 3259 ms, stages [("kind: subject", 0.99996185)], mentions ["Mike", "Kuiper", "Neptune", "Earth", "Planet Nine"], tier2 Some("\"Planet Nine\" → no cached brief")`** — not skipped at tier 1 (mentions non-empty and P(small talk) ≈ 0), reached tier 2 in shadow; active Kuiper evidence line **`oracle: subject "Planet Nine" — no cached brief; kind subject 1.00`** first in the delivery; active small talk (`yeah, sounds good / ok, thanks, talk later`) → **`Skip tier 2 in 159 ms, stages [("kind: subject", 0.64434844)], tier2 Some("nothing to look up")`**. A quieter earlier run of the same test: 1442 ms / 54 ms.
- Desktop: `just ts-desktop` regenerated `OracleDecision` (`stages`, `mentions`, `subject`, `tier2`); `cd crates/desktop/ui && bun run check` green (the pre-existing `packages/apps/src/renderer.tsx` error was a stale `packages/apps/node_modules` — the `@openuidev/react-lang` patch had not been re-applied since the rename; `bun install` there fixed it), `bun run test` **187 passed / 34 files** (`meeting-line.test.ts` covers the staged row incl. omitted stages, `store/meeting.test.ts` the tier-2 counter and summary).
- Not run: the headless smoke (plugin start logging the kind family; one `decision: warm-up done` at `Recording`) — the plugin-start line is unchanged code from step 4 and `warm_up` is exercised only by construction; the desktop app itself (`cargo tauri dev` unavailable here).

### Open

- **The small-talk sample is not skipped at tier 1.** `yeah, sounds good / ok, thanks, talk later` scores P(small talk) **0.36** (< τ 0.6) — the bench's 8/20 at 0.6 says the Bool head catches only the most obvious greeting/scheduling batches; the rest ride a 3 s tier-2 call to a Skip. Shadow data will show the actual skip rate; if it stays under ~40 %, variant 18/19 (`Nothing of substance…`, 100/112) with τ 0.5 is the next thing to bench, not another rewrite.
- The `b1..b4` publishes at 0.6/0.3 include two genuinely wrong ones in 112 (table above). The asymmetric gate accepts that cost by design; if shadow shows more, raise `tau_margin` on `meeting.brief` before touching the phrasing.
- `stages` names the Bool's winner `subject` when it means "not only small talk"; a reader of the ⚡ row may read it as a positive subject detection. Rename to `kind: talk` / `kind: small talk` if it confuses.
- Fixture labels: three icd scheduling batches carry model-noise `published` titles and the icd briefs leak forward (created in-meeting after the batch); a `created_at` on the fixture's briefs would let the bench exclude them instead of the hand note above.

## As built — steps 8–9 (2026-09-21): local training capture for the meeting oracle, `train` switch + RPC, export to the bench fixture

Brief: `/tmp/laya-design/omp-step89.md` (§11.4 as the spec; §11.1 items 1, 2, 5, 6 still hold). Commits on main: `52e6e7b` (`decision::training` schema + per-day writer + scrub, `FamilyGate.train`, `GetTrain`/`SetTrain`/`Capture`, RPC), `87de365` (meeting capture: every oracle-handled batch, `amend` with outcome and tap/dismiss), `2795f84` (export: fold → fixture / laya shapes, `t --decision-export`, `phrasing --fixture`), this note. Toolchain: `export PATH="$(just toolchain):$PATH"`.

### What one example looks like (`crates/decision/src/training.rs`)

One JSONL row per event under `<data_dir>/decisions/training/<prefix>/<YYYY-MM-DD>.jsonl`. The row below is the Kuiper batch from the model-gated test (`crates/meeting/tests/oracle.rs`), shadow, with a two-row cache; `text` shortened and probabilities rounded here, nothing else changed:

```json
{"row":"example","schema":"theuth.decision.v1","id":"01a0c549-fdd9-7970-b852-1e71bddc410c","ts":"2026-09-21T18:45:55.801400Z",
 "family":"meeting","harness":"1.9.2","model_sha":"891102d37268…","gate_version":3337696867,"mode":"shadow",
 "text":"them: So Mike was telling me about this at lunch. They were looking at the orbits of a few distant objects in the Kuiper belt, way out past Neptune, […]\nme: Has anyone actually seen it?\nthem: No, that's the thing. […]",
 "mentions":["Mike","Kuiper","Neptune","Earth","Planet Nine"],
 "questions":[
  {"family":"meeting.kind","type":"bool","instructions":"The speakers are only greeting, scheduling or saying goodbye","candidates":[],
   "probabilities":{"false":1.0,"true":0.0},"disposition":"shadow","gate_version":3337696867,"tier":1,"latency_ms":55},
  {"family":"meeting.brief","type":"choice","instructions":"which cached brief is the subject being discussed",
   "candidates":[{"id":"b1","text":"Mike's onboarding checklist"},{"id":"b2","text":"Earth observation satellite budget"}],
   "probabilities":{"b1":0.112,"b2":0.1184,"none":0.7695},"disposition":"shadow","gate_version":205688663,"tier":1,"latency_ms":46}],
 "tier2":{"subject":"Planet Nine","cached":null,"lookup":true},
 "verdict":{"action":"model","reason":"subject \"Planet Nine\", no cached brief"}}
{"row":"amend","id":"01a0c549-fdd9-7970-b852-1e71bddc410c","ts":"2026-09-21T18:45:56Z","outcome":{"published":{"id":null,"title":"Planet Nine"}}}
{"row":"amend","id":"01a0c549-fdd9-7970-b852-1e71bddc410c","ts":"2026-09-21T18:47:00Z","user":"tapped"}
```

- `Example`: `schema`, `id` (uuid v7 — time-ordered like a ulid, no new dependency), `ts`, `family` = the prefix, `harness` (`CARGO_PKG_VERSION`), `model_sha` (filled by the engine from the loaded checkpoint; `size:<bytes>` before the background sha lands, like the log), `gate_version` of the first question, `mode`, `text` (the `them:`/`me:` state exactly as passed to the engine — truncation is a hard error there, so whenever a question ran this is what the model read), `mentions`, `questions: [Asked]` (family, `type` choice|bool|score, instructions/statement, candidates **with titles**, probabilities as a map, disposition, reason through the shadow wrapper, gate_version, tier, latency), `tier2: {subject, cached, lookup} | {error}` when it ran, and **`verdict: skip | publish {id, title} | model {reason}`** — added beyond the brief's list so the export never re-derives `settle`.
- `Amend`: `id`, `ts`, then one of `outcome: { published: {id: <cache row | null for a researched brief>, title} | null }` or `user: tapped | dismissed`. `ignored` exists in the enum but is never written; the export shows it for the absence.
- Every row goes through `tools::secrets::redact_value` (new one-liner over the vault's `Secrets::redact_value`) before it is written: known vault values become `{{secret:NAME}}` placeholders. The store is append-only, one file per local day, created on the first row (`train: false` → no directory).
- `Store::info` counts example rows by parsing only the `row` tag per line (the scrubber round-trips through `serde_json::Value`, so key order on disk is not guaranteed).

### Switch and RPC

- `FamilyGate.train: bool` (serde default false) on every family; the YAML patch accepts `train` like `mode` — `decision.families.meeting: { train: true }` flips the three `meeting.*` families (tested in `lib.rs`).
- Engine: `GetTrain { prefix } → TrainingInfo { enabled, examples_today, examples_total, bytes, dir }`, `SetTrain { prefix, on } → Result<TrainingInfo, String>`, and `tell Capture { prefix, row }`, which drops the row unless a `<prefix>.*` family has `train` **now** (flipping it off stops writes mid-meeting). **`SetMode` and `SetTrain` both write the whole flow mapping `{ mode: …, train: … }`** — `write_nested_config_value` replaces the value, so `SetMode` alone would have erased `train` (and vice versa). Engine test `train_switch_writes_a_merged_prefix_patch_and_gates_capture`.
- RPC: `decision_training(prefix)` and `set_decision_training(prefix, on)` (both → `TrainingInfo`, exported in the TS bindings for step 10). To turn capture on today: `set_decision_training("meeting", true)` over the socket, or `decision:\n  families:\n    meeting: { mode: shadow, train: true }` in `config.yaml` (live on the next batch via the engine; the config line needs a restart).
- Headless smoke (this run, `t --headless --data-dir /tmp/tdata`, bun WebSocket client with the bearer token): `decision_training(["meeting"])` → `{enabled: false, examples_today: 1, examples_total: 1, bytes: 2114, dir: "/tmp/tdata/decisions/training/meeting"}`; `set_decision_training(["meeting", true])` → `enabled: true` and `config.yaml` reads `meeting: { mode: shadow, train: true }`; `…false` → `{ mode: shadow, train: false }`; `set_decision_training(["nobody", true])` → `-32000 no `nobody.*` decision family is configured`.

### Capture in `meeting::oracle` (`87de365`)

- `run` asks `GetTrain("meeting")` right after the mode check (one actor hop; nothing is cloned when off). When on, the two questions are kept as asked, and at verdict time — Skip, Publish and Model alike, shadow and active — the `Example` is built and `tell`-ed to the engine from the spawned oracle task; `Verdict.example: Option<ExampleId>` carries the id back to the actor.
- `Live.pending` carries the example id with each `(event, expected)`; `settle_pending` writes `amend { outcome }` per captured example with the turn's first publication (`(cache row | null, title)` — the same approximation `outcome()` already makes when several batches share one turn) and remembers the published topics (`Live.published_topics`, pushed beside `published` in `meeting_topic`) against those example ids in `Live.captured`. An oracle publish (`Action::Publish`) records `(topic, example)` directly. `TapTopic` / `DismissTopic` call `Live::react`, which writes `amend { user }` for every captured example behind that topic and forgets the topic; `oracle::amend` is a spawned `tell`, so neither handler waits on the engine.
- Model-gated `tests/oracle.rs` (this run): default config → `Verdict.example` is `None` and `decisions/training` does not exist; after `SetTrain(meeting, true)`, the shadow Kuiper batch with a two-row cache writes the row above (mentions, both questions with title candidates `b1`/`b2`, tier-2 `subject: "Planet Nine"`, verdict `model`), then two `amend` rows (`outcome.published: Planet Nine`, `user: tapped`) land on the same id; `GetTrain` counts 1/1. The first cache I tried (`Kuiper belt survey`) was published at tier 1 (P 0.77, margin 0.62) and the batch never reached tier 2 — correct pipeline behaviour, so the test's cache rows only share a mention (`Mike`, `Earth`) with the batch.

### Export (`2795f84`)

`t` has no subcommand tree (flat flags: `--secret-set`, `--link`, …), so the export is **`t --decision-export <prefix> [--since YYYY-MM-DD] [--out FILE] [--format fixture|laya]`**, implemented in `decision::training::{fold, label, fixture, laya, export}` (the binary only parses flags). `fold` keys `amend` rows to their example (last outcome / last reaction wins; an amend without its example is dropped).

Label derivation as implemented (`training::label`, table-tested in `label_derivation_table`):

| input | `kind` | `expected_brief` | `source` |
|---|---|---|---|
| `published` = the outcome amend when a turn ran, else a `publish` verdict, else none | — | — | — |
| tier 2 `subject: null`, nothing published | `small_talk` | none | `tier3` if an outcome row exists, else `tier2` |
| tier-1 skip, no tier 2, nothing published | `small_talk` | none | `oracle_only` |
| a tier-2 subject, or anything published | `named_subject` | the published **cached** brief's candidate id (`b1..`, matched by title among the brief question's candidates); a researched brief or an unlisted title → none | `tier3` / `tier2` / `oracle_only` |
| tier 2 failed (or nothing heard) and nothing published | `general` | none | `tier3` / `oracle_only` |
| user `tapped` | as above | as above (confirmed) | `user` |
| user `dismissed` | as above | **none** | `user` |

- `--format fixture` (default) = `{ _doc, batches: [{ id, source, kind, state, published: [{title, cached}] | null, expected_brief, candidates: [{id, title, aliases: []}], mentions, subject, cached, user }] }` — the field names `examples/phrasing.rs` reads (`state`, not `text`; `published` as the fixture's list), plus §11.4's `subject`, `cached`, `user`, and `source` = the label's origin (the hand-built fixture used `source` for the meeting name; the bench never reads it). `phrasing --fixture FILE` replaces the built-in fixture; run this session on the exported test row: `1b` K4 `b1..bK` → `1/1`, `none` accepted right, p50 36 ms.
- `--format laya` = the `LocalLLaMA/typed-decisions` row columns (checked against the dataset card): `id`, `workflow`, `state`, `questions` (`kind` as `noul` with the statement, `brief` as `choice` with `criteria` = candidate id → title + `none`), `gold` (`{label, probability_true}` / `{label, probabilities}`, one-hot on the derived label — we have no soft labels), `kind__label`, `brief__label`, plus `label_source`. §11.1(5)'s single-question `question/oracle/label` envelope was not used: the fine-tune notebook consumes the dataset shape, and one batch is one multi-question case there.
- Smoke: `t --data-dir /tmp/tdata --decision-export meeting --out /tmp/exported.json` → `wrote 1 examples …`; the batch reads `kind: named_subject, source: user, user: tapped, published: [{title: "Planet Nine", cached: null}], expected_brief: null, subject: "Planet Nine"`; `--format laya` → `kind__label: false, brief__label: "none"`; `--since 2026-09-22` → 0 batches; `--format csv` → `unknown format "csv": fixture | laya`, exit 1.

### Deviations from §11.4 / the brief

- **`verdict` is in the example** (not in the brief's field list): the export needs the oracle's own act for `oracle_only` rows and for `published` when no turn ran; recomputing `settle` offline would drift.
- **`text` is the state as passed, not "after truncation"**: the engine treats truncation as a hard error (`Reason::Truncated`), so a truncated batch never has model numbers; the row's `reason` says so.
- **Ids are uuid v7**, not ulid — same ordering property, no new crate; `ExampleId` follows `LogId`.
- **`published.id` is nullable**: a brief the turn researched has no cache row at publish time (its own row is not a cached pick, as `outcome()` already treats it); the fixture's `published[].cached: null` is the same convention.
- **`Brief.id` for the oracle's own publish is the cache row id** (not `b1`); the export maps it back to the candidate id by title.
- **`small_talk` also for a tier-1 skip without tier 2** — the brief's rule (`tier-2 subject: null ∧ nothing published`) would have made every skipped small-talk batch `general`; §11.4(1) calls those the negative examples.
- **`source` on the fixture batch** carries the label origin as the brief asks; the hand-built fixture's meeting-name `source` is unread by the bench.
- No `ignored` row is ever written (derived at export), as specified. The Settings section (step 10) and the uploader/relay (step 11) are untouched; `train_keep_local` is not read yet — nothing deletes.

### Checks

- `cargo check --workspace --all-targets` clean (the `block v0.1.6` future-incompat note is pre-existing). `cargo test -p decision -p meeting -p rpc`: `decision` **58 unit** (52 → 58: schema round trip incl. amend shapes and tier-2 error, per-day append-only store with a fake vault proving `hunter22hunter22` → `{{secret:PASS}}`, label table, fold, export shapes; engine `train_switch_…`; lib `train` prefix patch) + `tests/model.rs` 2; `meeting` **38 unit** + `tests/oracle.rs` 1 model-gated (extended as above); `rpc` **17**. `cargo clippy -p decision -p meeting -p theuth --no-deps --all-targets` clean; `cargo clippy -p tools` fails on three **pre-existing** lints in `pty.rs`/`registry.rs` (`obfuscated_if_else`, `type_complexity`, `too_many_arguments`) untouched here — `secrets.rs` gained one function.
- Headless RPC smoke and the export smoke as recorded above; `phrasing --fixture /tmp/exported.json --only 1b` ran against the checkpoint.
- Not run: a real meeting with `train: true` (no meeting was recorded during this build); the desktop app.

### Open

- Several pending batches in one turn all receive the turn's first publication as `outcome.published` — the same approximation `outcome()` makes for the decision log. If it skews the `brief` labels, attach publications to the batch whose `[meeting hh:mm:ss]` line the model answered (the `batch_id` is in the event already).
- `captured` topics that expire untouched stay in `Live.captured` until the meeting ends (bounded by topics per meeting). Fine for v1; drop them in `topics_gone` if a meeting ever grows thousands of topics.
- `harness` reads `1.9.2` from the workspace at build time; the training rows and the decision log will disagree on version only across a release boundary.

## As built — step 5 (2026-09-21): `browser_run` — the System-1 loop on `cklxx/laya-browser` v10

Brief: `/tmp/laya-design/omp-step5.md` (§4.1 as the spec). Worktree branch `feat/browser-systemone` off main `07a685a`. Commits: `258aa07` (decision: checkpoints per family, `AskMany`, `head_max_len` from `rl_agent_config.json`), `113dc38` (`scripts/fetch-laya-browser.sh`, `decision-browser/` in `release-models.sh` REQUIRED — run: `~/.theuth/models/decision-browser/{model.safetensors 842,609,220 B, encoder/config.json, rl_agent_config.json, tokenizer/tokenizer.json, tokenizer/tokenizer_config.json}`), `cd80ef8` (`browser::systemone` observe/render + parity test), `f90e335` (`browser_run`: loop, gate/stop rules, shadow/active, tier-3 hand-back, `tests/live.rs`), `a1156e2` (`decision.batch_forward`, default `false` — see latency), `653cc69` (`examples/systemone_suite.rs`), this note. `crates/meeting` untouched; no merge, no push.

### Checkpoint per family and `AskMany` (`258aa07`, `crates/decision`)

- `decision.checkpoints: { default: <dir>, browser: <dir> }` (`DecisionConfig::checkpoint_dirs`; `default` = the old `model_dir`, which still works; `browser` defaults to `<data_dir>/models/decision-browser`). `FamilyGate.checkpoint: Option<String>` (`checkpoint()` → `default`), `FamilyGate::browser(tau)`. The plugin spawns **one `Engine` per checkpoint dir**: `default` stays `DECISION_ACTOR`, the others `decision/<name>` (`actor_name`); `Engine::with_checkpoint(name, dir)`. An `Ask`/`AskMany` sent to any engine is **routed** (`route()`: the first ask's family → its gate's checkpoint → sibling actor, `DelegatedReply` forwarding) so callers keep using `DECISION_ACTOR`; a batch whose families sit on different checkpoints answers the strays `HardError("family … is on checkpoint …")` — fail closed, named. `SetMode`/`SetTrain` relay to siblings; each engine loads lazily and idle-unloads as before (~0.9 GB each on Metal when both are resident).
- `AskMany { asks } -> Vec<Decision>` (order kept, disabled/off families escalated in place): `run_batch` does gate lookup per question, one session, one `build` per question, the forward, then one `decide` + one `AskRow` per question; the budget is the largest asked and a timeout escalates every question (late rows still logged). `Ask` is the one-element case of the same core.
- `head_max_len` is read from the checkpoint's `rl_agent_config.json` (v10: `max_len` 1024, `head_max_len` 768, temperatures `[5.03, 1, 1]`) and `build_sequence` honours it; on v10 the state is right-cut by design on a full page.
- Two per-family gate facts the browser families need: **no `none` control** (`FamilyGate.none = false` — the option set is closed, jev never offers "none of these"; the classic families keep `none`), and a **right-cut state is not a hard error** (`cut_ok = true`; `decide()` uses `seq.markers_lost || (seq.state_cut && !gate.cut_ok)`). Defaults (`default_families`): `browser.operation` τ 0.5, `browser.target` / `browser.type_target` τ 0.25, margin 0, budget 2 s, checkpoint `browser`, **mode shadow**.

### `browser::systemone` (`cd80ef8`) — jev's request shape, verbatim

- **Request = jev-ultrafast `model.choose()` after cklxx's format-v2 transform** (`v10`'s `laya_fmt`). State = Python `json.dumps(state, ensure_ascii=False)` of `{"page": {"url", "title", "text"[:1500]}, "recent_actions": [...]}` (`", "`/`": "` separators, C0 controls `\n\r\t\b\f` else `\u00xx`, non-ASCII raw); instructions = `json.dumps(dict)` with `ensure_ascii=True` (`\uxxxx`, surrogate pairs above the BMP): `{"goal": G, "rules": RULES}` for `operation`, `{"goal": G, "operation": "CLICK"|"TYPE_TEXT", "rules": [RULES, TARGET_RULES]}` for the targets. `py_json_str` / `py_repr` implement Python's escaping (unit-tested). `RULES` = jev `questions.NEXT_ACTION`, `TARGET_RULES` = jev `questions.TARGET`, `TEXT_VALUE` = jev's text-helper system prompt, all verbatim.
- Operation criteria in jev's order, texts verbatim from jev `model.py` + `snapshot.js`: CLICK (if any element), TYPE_TEXT (if any editable), SCROLL_DOWN (if more below), SCROLL_UP (if scrolled), WAIT, DONE, BLOCKED. **No SELECT**.
- Option text (cklxx `compact()`): `"[i] label (role)"` + `" = 'value'"` (Python repr, value cut at 30) when the value is non-empty + `" checked=…"`/`" selected=…"`/`" expanded=…"` when present; candidate id = `i` (1-based in the window) so laya renders `"i: [i] label (role)"` = upstream `render_options`. The click list names an editable `"[i] Open <label> (role)"` (jev's second, click-kind action for every editable); the type list uses the plain label. History `action` for a click on an editable = `"Open <label>"`.
- `recent_actions` entries jev-shaped `{action, kind: click|fill|scroll|wait, text: typed value|null, page_changed}` — **last 6** (jev sends 10; brief said 6). `page_changed` = the `(url, title, text)` fingerprint differs at the next observe.
- **Reader (`OBSERVE_JS`)** = jev's `snapshot.js` over obscura's stamped refs: every `[data-obscura-ref]` element (`browser_interactive_elements` stamps them in DOM order) minus password/file/hidden inputs, `<select>`, hidden (attribute, inline style, `aria-hidden`/`inert` ancestors — obscura has no layout), disabled; must have one of jev's 14 roles (`role()` port: BUTTON/SUMMARY→button, A→link, TEXTAREA/contenteditable→textbox, INPUT by type incl. search→searchbox, number→spinbutton). Label = jev `name()` (aria-labelledby → aria-label → labels → button value → alt → text content (not INPUT) → title → placeholder), whitespace-collapsed, else the role. `editable` = not readonly ∧ (textbox|searchbox|spinbutton ∨ combobox INPUT/TEXTAREA). Text = visible text nodes in DOM order joined by `\n`, capped at 6000 chars in JS, then the 1500-char **text window**.
- **Logical viewport.** obscura lays out nothing and `window.scrollY` never moves, so `Scroll { elements, text }` emulates one: a step offers a window of **60 elements** (`WINDOW`) renumbered 1..n; `SCROLL_DOWN` advances 40 elements / 1200 chars and also sends `browser_scroll down` (fires scroll events); `SCROLL_UP` reverses. `more_below` / `scrolled` gate the two scroll criteria.
- **Parity test** (`tests/systemone.rs::renders_cklxx_sample_request_byte_for_byte`): the Wikipedia observation reconstructed from `code/sample_request.json` renders to the **byte-for-byte** instructions, option lines (`id: text`, in order, 6 / 58 / 12), state and `choice question: …` head of `tests/fixtures/laya-browser-sample_request.rendered.json`, produced with laya's `common.py` `serialize_state` / `render_options` + `json.dumps(instructions)` (v2 transform = `text[:1500]`). **Pass.**
- **Model-gated** (`v10_answers_type_text_into_the_search_box`, release): `browser.operation → TYPE_TEXT 0.984` (CLICK 0.014), `browser.target → [2] 0.596` over 58 candidates (no `none`), `browser.type_target → [2] 1.000` — the file's `expected`. First step 11.9 s (cold load + Metal shader compile), warm step **402 ms** as one `AskMany` (load average 26).

### The loop and `browser_run` (`f90e335`) — `crates/browser/src/systemone.rs`, `tools.rs`

- **`Ctx::run(session, goal, max_steps)`**: observe (`browser_interactive_elements` — also the hand-back listing — then `browser_evaluate(OBSERVE_JS)`) → `render` → one `AskMany` (budget from `GetMode{browser}.budget_ms`) → operation accepted? target accepted? → execute through the existing session tools: CLICK `browser_click{ref}`; TYPE_TEXT value = `quoted_value(goal)` (`'…'`, `"…"`, `“…”`) else one `SideCall{Escalate, TEXT_VALUE, 3 s}` on `agent::Agents` → `browser_fill{ref, value}` (≤ 80 chars); SCROLL_* `browser_scroll` + window move; WAIT = idle 2 s → push `RecentAction` → repeat. **Stop**: `DONE`, `BLOCKED`, step cap (default and hard cap 25), 60 s wall clock, **three identical consecutive decisions** (checked before executing the third), below gate (operation or target), error. Result `Run::render()`: `browser_run (active): N step(s) in T ms`, one line per step (`step 2: CLICK 0.99 [6] "Search" (button) 0.35 · 1246 ms + 6582 ms`), `stopped: …`, `page: url — title`, `Continue with the browser_* tools` unless DONE, then the element listing (tier 3 continues in the same model turn).
- **Shadow**: decide one step, do not act; `Ctx.pending: Mutex<HashMap<SessionId, Pending{operation log_id, target log_id, expected (op, ref)}>>`; the tool result is the trace + listing. `Ctx::call` (every browser tool goes through it) calls `settle()` on the session's next **acting** obscura call — `browser_click/fill/type/scroll/wait_for*/navigate/back/press_key/select_option` (`actual_action`); reads settle nothing, so the loop's own observe never labels itself — and `Pending::report` `tell`s `Report{Confirmed | Corrected{actual}}` for both rows (target row: op and ref must both match). A new `browser_run` supersedes an unlabelled pending decision. No `agent` change was needed: the pending map lives in the browser plugin, as the brief hoped.
- **Modes**: `decision.families.browser.mode` (prefix patch; default shadow). Off → `tools::all` skips `RunTool` (`configured_mode` reads `rt.section("decision")` at plugin start) and a call that reaches `run` with effective off answers `Stop::Error`. `browser_run` is `Risk::Mutating`; the description says it may perform up to `max_steps` actions / 60 s in the session's headless browser under one approval.
- `TODO(capture)` at the call site in `RunTool::call`. Main's capture writer (`decision::training`, `52e6e7b`) is in this tree but its `Example` is meeting-shaped; the browser `Example` to write, per §4.1 item 7 / §11.4 (family `browser`, one per step): `{ log_ids: [operation, target?], goal, observation: Observation (url, title, text window, elements [{index, ref, label, role, value, editable}], recent_actions, scroll), asks: the three Question::Choice as sent (instructions, candidates, state), probabilities: per family, decision: {op, target index, value?}, executed: bool, outcome: Confirmed | Corrected{op, ref} (shadow: the model's next call; active: the tier-3 correction after a hand-back, else DONE = confirmed), stop: Stop, step_ms }` — the DAgger label is the correction.
- `tests/live.rs::drives_fixture_page` (scripted oracle standing in for the head, fixture page on a local port): **active** types the quoted value, submits, sees the echo, DONE in 3 steps (744 ms); **shadow** decides one TYPE_TEXT step, does not act (`browser_read` shows no echo), the hand-back carries `ref=e1`; a read (`browser_elements`) settles nothing, the model's `browser_click e2` labels both rows `corrected` (`Stats` agreement 0.0). Pass, both halves.
- Cargo: `browser` gains `decision`, `agent` (the `SideCall` lookup — the edge `meeting` already has; `agent` does not depend on `browser`), `parking_lot`; dev-deps `tempfile`, `tracing-subscriber`. `decision`: no new deps.

### Latency: the `AskMany` padding finding (`a1156e2`)

`examples/bench.rs` (b2)/(b3), M3 Max, Metal f16, browser checkpoint, 20 warm runs, load average 19–23 (`/tmp/laya-design/bench-step5-browser-batch-vs-seq.txt`): the browser step = 3 rows of **S 476 / 1024 / 583** tokens (6 / 45 / 10 candidates, ~1.5k-char state, state cut on the 45-candidate row).

| forward | padded tokens | p50 | p95 |
|---|---|---|---|
| one batch, 3 rows padded to S 1024 (`batch_forward: true`) | 3072 | **1034 ms** | 1116 ms |
| three single-row forwards, each at its own S (`batch_forward: false`, **default**) | 2083 | **569 ms** | 577 ms |

The forward is close to linear in padded tokens on our candle/Metal path, so padding two rows to the third's length costs more than the batch saves. `decision.batch_forward` (default `false`) keeps the one-batch path selectable; no deeper optimisation (unpadded packing, fused kernels — follow-up (3)) done here. Single rows for reference: K2 S339 73 ms, K10 S729 182 ms. **Still over the §4.1 ≤ 300 ms target**: the 45-candidate row alone is a ~1024-token forward.

### Suite (`653cc69`, `examples/systemone_suite.rs`) — cklxx's 16 tasks × 3 runs, active mode

Task names, start URLs, goals and outcome checks are copied from their `code/apps/browser_suite.py` (hub, 2026-09-21); the two state checks (dropdown value, checkbox checked) read the DOM. Run 2026-09-21, Metal f16, `batch_forward: false`, **load average 17 → 65** from other sessions' builds during the run (`/tmp/laya-design/suite-step5-16x3.txt`, runs only: `…-runs.txt`); the suite's step cap is 20, then 1.5 s settle before judging.

| task | runs | steps | p50 decide ms | below gate | stop / why |
|---|---|---|---|---|---|
| wiki-einstein | ✗✗✗ 0/3 | 1/1/1 | 901 | 0 | DONE 0.60 after clicking the searchbox |
| wiki-search | ✗✗✗ 0/3 | 4/4/4 | 907 | 0 | repeated: typed the query, then "Open Search Wikipedia" ×3 (no autocomplete without layout; never chose the Search button) |
| hn-new | ✓✓✓ 3/3 | 1/1/1 | 627 | 0 | DONE |
| hn-login-page | ✓✓✓ 3/3 | 1/1/1 | 625 | 0 | DONE |
| gh-issues | ✗✗✗ 0/3 | 1/1/1 | 607 | 3 | below gate: CLICK target P 0.03 on [25] "Solutions" (flat over the offered window) |
| py-downloads | ✓✓✓ 3/3 | 1/1/1 | 1179 | 0 | DONE |
| books-travel | ✓✓✓ 3/3 | 1/1/1 | 754 | 0 | DONE |
| books-open-book | ✓✓✓ 3/3 | 1/1/1 | 737 | 0 | DONE |
| internet-dropdown | ✗✗✗ 0/3 | 2/2/2 | 388 | 3 | below gate; no SELECT operation, `<select>` not offered — it clicked the site's link away |
| internet-checkbox | ✓✓✓ 3/3 | 1/1/1 | 374 | 0 | DONE |
| books-page2 | ✗✗✗ 0/3 | 1/1/1 | 854 | 3 | below gate: "next" is element 61+, past the 60-element window; it did not SCROLL_DOWN |
| quotes-tag-love | ✓✓✓ 3/3 | 1/1/1 | 785 | 0 | DONE |
| hn-past | ✓✓✓ 3/3 | 3/3/3 | 890 | 0 | repeated: goal reached on step 1, kept clicking "past" on `/front` |
| ddg-search | ✓✓✓ 3/3 | 3/3/3 | 1191 | 0 | DONE (type, Search, Search, DONE) |
| arxiv-search | ✗✗✗ 0/3 | 1/1/1 | 570 | 3 | below gate: CLICK target P 0.03 |
| flights | ✗✗✗ 0/3 | 1/1/1 | 1222 | 3 | below gate on Google's consent interstitial |

**Tasks passing every run 9/16 = 56 %** (runs 27/48 = 56 %) against **their v10 50 %**, and bimodal exactly like theirs: 9 pass 3/3, 7 fail 3/3, no mixed task. Every fail is deterministic (same decision, same P, three times) — the head is deterministic given the rendering, so the gate and the reader, not luck, decide a task. Below-gate hand-backs (15 of 48 runs) are what tier 3 would pick up in the daemon; in the suite they count as fails. **Step latency over 99 decisions: p50 828 ms, p95 1246 ms, max 1557 ms** — over ≤ 300 ms, and inflated by a load average of 47–65 during the second half (the bench's 569 ms at load 20 is the clean number for the same shape). Histogram: 200–300 ms 4, 300–500 10, 500–800 33, 800–1200 43, 1200–2000 9. Suite wall 379 s.

### Deviations from §4.1 / the brief

- **No SELECT operation**, `<select>` elements not offered (jev has one; this loop's session tools have `browser_select_option` but the head's criteria set was kept to what cklxx trained on without a value channel). Costs `internet-dropdown`.
- **`recent_actions` = last 6** (brief), jev sends 10.
- **No `none` control** on the browser families (closed option set, as trained); `cut_ok` — a right-cut state is normal on v10.
- **Logical viewport** (60 elements / 1500 chars, SCROLL_* moves it) instead of jev's real viewport: obscura has no layout. Costs `books-page2` and hides autocomplete flows (`wiki-search`).
- **WAIT = idle 2 s** (`tokio::sleep`), not a settle primitive — obscura has none; the isolate pumps timers/fetches between commands.
- **TYPE_TEXT value**: quoted string in the goal, else the side call; the brief's "the goal's object after for/into/as" heuristic was not added — the side call covers it, and every suite goal that reached TYPE_TEXT quotes its value (the suite runs without an agent, so the side call would have failed the step).
- Model-gated latency in `tests/systemone.rs` is printed, not asserted (the bench's job).
- Headless smoke (`t --headless`, `list_tools` with `browser` mode active vs off): see Checks.

### Engine merge notes for main (`crates/decision/src/engine.rs`)

Main's training-capture work (`52e6e7b`+) touches the same file. What this branch changed: `Engine` gains `checkpoint`, `default_dir`, `with_checkpoint(name, dir)`, `route()`/`siblings()`; the `Ask` handler became `run_batch(Vec<Ask>)` + thin `Ask`/`AskMany` handlers returning `DelegatedReply` (forwarding to the sibling engine when routed); `decide()` uses `seq.markers_lost || (seq.state_cut && !gate.cut_ok)`; `escalated()` helper; `merge()` (in-place escalations + ran rows); `SetMode`/`SetTrain` relay `tell` to siblings; `FamilyGate` lost `Copy` (`.copied()` → `.cloned()`); `run_batch`'s forward is `batch_forward ? o.run(&seqs) : one o.run per row`. Tests: `FamilyGate` literals need `checkpoint: None, none: true, cut_ok: false`; `Sequence` literals need `state_cut`, `markers_lost`; the `browser.wait` family in one test became `browser.target`; `ask_many_is_one_forward…` sets `batch_forward: true`. `lib.rs`: `checkpoints` config, `DEFAULT_CHECKPOINT`/`BROWSER_CHECKPOINT`/`actor_name`, `checkpoint_dirs`/`checkpoint_dir`, `batch_forward`, `mode_for` resolves the prefix's checkpoint dir (signature unchanged for meeting); the plugin spawns one engine per checkpoint. `gate.rs`: `FamilyGate::browser(tau)`, `checkpoint()`, `none`, `cut_ok`. `Cargo.lock` gains the `browser` edges only.

### Checks

- `cargo check --workspace --all-targets`: green except the pre-existing `nodes` lib-test error (`include_str!("../../../bindings/methods.json")`, gitignored, written by `cargo test -p rpc`; noted in the follow-ups (1)+(2) as-built).
- `cargo test --release -p decision -p browser`: `decision` **58 unit** (52 → 58: checkpoint dirs / `actor_name`, `AskMany` one forward + per-question rows + closed sets, routing to a sibling checkpoint / fail-closed, `head_max_len` from config, `collate` floor) + `tests/model.rs` 2 model-gated; `browser` **19 unit** (9 → 19, `systemone::tests`: Python `json.dumps`/`repr` escaping, state = `serialize_state`, instructions = `json.dumps` of jev's dicts, option text per `compact()`, op ids round trip, `quoted_value`, reader JSON parse, last-6 `recent_actions`, window renumbering + operation criteria) + `tests/systemone.rs` 2 (parity byte-for-byte, v10 model-gated) + `tests/live.rs` 1 (active + shadow on the fixture, scripted oracle). All pass. The scripted oracle originally counted steps per forward and broke when `batch_forward` went to `false` (three forwards per step); it now counts per operation question — folded into `a1156e2`.
- `cargo clippy -p decision -p browser --no-deps --all-targets`: clean.
- **Headless smoke** (`target/debug/t --data-dir /tmp/theuth-hl-step5-<mode> --headless`, `config.yaml` with `decision: { enabled: true, families: { browser: { mode: <mode> } } }`, `list_tools` over the JSON-RPC HTTP endpoint with the data dir's bearer token): `active` → **91 tools, `browser_run` present**; `off` → **90 tools, `browser_run` absent**; the 16 other `browser_*` tools identical in both.

### Open

- **Latency is 2–3× the §4.1 target** even in the faster mode (569 ms clean, 828 ms p50 under load): the 45–60-candidate target row is a ~1024-token forward on its own. Levers, in order: cut `head_max_len`/`max_len` padding (the state is already right-cut; K-dependent S), a smaller `WINDOW` (fewer candidates → shorter row; costs recall like `books-page2`), follow-up (3)'s fused kernels, unpadded packing. None started.
- **The 7 failing tasks are deterministic** and split into reader ceilings (`internet-dropdown` no SELECT, `books-page2` window, `wiki-search` no autocomplete/layout, `flights` consent wall) and head ceilings (`gh-issues`/`arxiv-search` flat target P 0.03 → below gate; `wiki-einstein` DONE 0.60 after one click). In the daemon the below-gate ones hand back to tier 3 — the shadow log will show how often the model then succeeds; only then is a `SELECT` operation or a larger window worth the drift from cklxx's training distribution.
- **Gate thresholds** (0.5 / 0.25) were not tuned by the suite: every accepted target had P ≥ 0.35 and every refused one P ≤ 0.11, so nothing between 0.11 and 0.35 was observed — the bench cannot separate 0.25 from 0.15 or 0.3 yet.
- **Capture** for family `browser` waits on main's writer with the `Example` above; `TODO(capture)` in `RunTool::call`.
- Settings pane for the browser mode: step 10 (config key until then). mmBERT (v10s, 62 %) needs a second backbone in `candle/model.rs`.
