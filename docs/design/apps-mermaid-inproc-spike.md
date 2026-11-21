# Apps `check_mermaid` in-process: feasibility spike

Branch `spike/apps-mermaid-inproc` (cut from main ccd3789ca). Question: can the apps helper's `check_mermaid`
op (`packages/apps/src/mermaid-check.ts`: jsdom + `mermaid.parse()`, today inside the 71 MB
`theuth-app-parser-<hash>` child) run in a bare `deno_core` 0.412 `JsRuntime` with no jsdom and give the same
answers for every diagram type?

## Verdict

**Works, with caveats.** mermaid 12.0.0's parse path runs DOM-free once `dompurify` is aliased to an identity
shim: one 2.86 MB runtime-neutral bundle (5.37 MB untrimmed), a 100-line polyfill (TextEncoder/TextDecoder, a
refusing `URL`, `structuredClone` from deno_core, timers), in a bare deno_core 0.412 isolate on its own thread.
**147/148 corpus answers are byte-identical to the jsdom helper path; the 148th differs only in the JS engine's
own TypeError wording (JSC vs V8), with the same `ok` and `line`.** That covers all 41 diagram ids mermaid 12.0.0
registers. Runtime build 62 ms, first parse of any type ≤ 35 ms on a fresh runtime, warm p50 1.6 ms (debug
build of the Rust side; V8 itself is the prebuilt release library). Caveats: (1) mermaid's own parsers are
super-linear on some shapes, and V8 runs them ~1.6× slower than JSC: a 32 KB class diagram (1900 inheritance
lines) takes **3.0 s under V8** vs ~1.9 s in the bun helper, so the 2 s deadline times it out; recommend a 5 s
warm deadline for mermaid (§4, §6); (2) loss of process isolation: a V8 fatal error aborts `t` (§6); (3) one
more V8 isolate in `t` (~60 MB RSS after load, ~110 MB after every diagram type has loaded its chunk).

## 1. The DOM question

Reading confirmed by running: mermaid's module body touches no DOM (every use is `typeof document`/`window`
guarded), `mermaid.parse` → `Diagram.fromText` → the diagram's lazy loader → parser + db. The only DOM user on
the parse path is DOMPurify: `sanitizeText` (common/common.ts) calls `DOMPurify.sanitize` on labels and
`setupDompurifyHooks` calls `DOMPurify.addHook` twice. Without a `window`, the real `dompurify` export has
`isSupported: false` and no `sanitize`, so every label-bearing diagram would throw.

**Decision: alias `dompurify` to an identity shim** (`packages/apps/src/inproc/dompurify-shim.ts`: `sanitize`
returns its string input, hooks and config are no-ops). Justification: nothing is rendered, so nothing is
sanitised for display; the sanitised strings only land in the diagram db, which `parse` discards; the corpus
contains HTML labels (`<b>`, `<script>`, `<img onerror>`, `<br/>`), all answer identically. linkedom (the
documented fallback) was not needed and is not bundled. The bundle build fails if `dompurify`, `jsdom`, `react`
or `react-dom` appear, or on any `node:`/`bun:` specifier or bare `require(` call.

## 2. Corpus and parity

`packages/apps/test-inproc/mermaid-corpus/`: `valid.ts` (one valid source per registered diagram id),
`corpus.ts` (derives two invalid variants per type plus edge cases), `reference.ts` (records the CURRENT
implementation's answers — `src/mermaid-check.ts`'s `checkMermaid`, jsdom installed exactly as the helper does,
under bun — into `golden.json`), `parity-vm.ts` (runs the bundle in a bare `node:vm` context).

Diagram ids, enumerated from `node_modules/mermaid/dist/mermaid.core.mjs` `addDiagrams` (not guessed): `error`,
`---` (front-matter guard), and the 37 lazily registered detectors c4, flowchart-v2 (`flowchart`/`graph`),
agentflow, swimlane, er, gitGraph, gantt, info, pie, quadrantChart, xychart, requirement, sequence,
classDiagram, stateDiagram (v2; `stateDiagram` without `-v2` goes to the same detector), journey, flowchart-elk,
timeline, mindmap, kanban, sankey, packet, radar, block, treeView, architecture, eventmodeling, ishikawa, venn,
treemap, usecase, wardley, cynefin, railroad, railroadEbnf, railroadAbnf, railroadPeg. No zenuml (an external
plugin, not in mermaid 12.0.0). The corpus has 41 type keys (`graph` and `stateDiagram-v1` as separate keys
for the two spellings).

Corpus: **148 sources** = 41 types × 3 (valid; garbage line inserted as line 2; garbage line appended as the
last line) + 25 edge cases: empty, whitespace-only, unknown keyword, `error`, front matter (valid, invalid
body, unclosed, bad YAML), `%%{init}%%` (valid, bad JSON), comments, HTML labels ×3, unicode ×2, CRLF, leading
blank lines, 32 KB flowchart over the 500-edge limit (+ error at end), 32 KB flowchart under the limit
(+ error at end), classDef/style, click callbacks + URLs, subgraphs, an unclosed sequence loop, a bad gantt date,
a negative pie value, accTitle/accDescr, a TypeError inside timeline's db, ishikawa's lenient grammar.

Grammars that swallow the generic garbage line as free text (stateDiagram, timeline, treeView) get a line they
do reject (`BAD_LINE` in `corpus.ts`, found by probing). ishikawa accepts every line tried; its invalid
variants are therefore `ok:true` in the golden file too (a property of mermaid, recorded as such).

### Results

| runner | exact | engine wording | mismatch |
|---|---|---|---|
| bundle (trimmed, 2.86 MB) in bare `node:vm` (bun/JSC), `parity-vm.ts` | 147/148 | 1 | 0 |
| bundle (untrimmed, 5.37 MB) in bare `node:vm` | 147/148 | 1 | 0 |
| bundle (trimmed) in deno_core 0.412 / V8, `tests::corpus_matches_golden_and_measure` | **147/148** | 1 | **0** |

Per type: every one of the 41 types matches on all three of its cases in every runner (ok flag, line, full
message). The single non-exact case, `edge/timeline-typeerror` (`timeline\n: x`), is a TypeError thrown inside
mermaid's timeline db; its message is the engine's own text and the line is `null` in both:

- golden (jsdom path, bun/JSC, unminified): `undefined is not an object (evaluating 'rawTasks.find((task) => task.id === currentTaskId - 1).events')`
- bundle under bun vm (JSC, minified): same sentence with minified identifiers (`'jW.find((i)=>i.id===SVr-1).events'`)
- deno_core (V8): `Cannot read properties of undefined (reading 'events')`

The shipped helper is `bun build --compile --minify`, so even today its text for this case is the minified one;
it was never stable. Nothing else in the corpus depends on the engine.

Things found on the way, all harness-only:

- The bun `node:vm` harness needs a same-realm `structuredClone`: the host's returns host-realm `Map`s and
  mermaid's usecase db checks `instanceof Map` (`Cannot commit an incomplete usecase model`). deno_core's
  `core.structuredClone` runs in the isolate's own realm; the V8 run confirms it.
- Five of my first "valid" sources were wrong syntax (usecase, railroad ×3, eventmodeling); fixed by probing
  the jsdom path, then golden re-recorded. Re-recording is deterministic (no diff on a second run).

## 3. Bundle

`packages/apps/src/inproc-mermaid.ts` → `globalThis.theuthMermaid = { call(line) → Promise<string>, version }`,
`protocol.ts` `MermaidRequest` in, `MermaidResponse` or `ParserErrorResponse` out (same validation messages as
`helper.ts` for this op). The DOM-free half of the check (`answer`: `parse`, then the `line N` regex and the
400-char cap) moved to `src/mermaid-core.ts` and is shared with `mermaid-check.ts`, so both paths format errors
from one function. `scripts/build-inproc-mermaid.ts`: `Bun.build`, `target: "browser"`, `format: "iife"`, one
output file, `process.env.NODE_ENV` defined, an `onResolve` plugin for the aliases.

- **Lazy chunks stay lazy.** Bun turns mermaid's string-literal `import("./chunks/…")` into
  `Promise.resolve().then(() => (init_xxx(), exports_xxx))`: every chunk is *parsed* at load (one file), but a
  diagram's module bodies (its parser, langium services, …) *evaluate* on the first parse of that type. That is
  what the per-type first-parse cost in §4 is.
- **Globals needed** (found by running in a context with only ECMAScript built-ins): `TextEncoder`/`TextDecoder`
  (vscode-jsonrpc's browser RIL, evaluated inside every langium-based chunk: pie, packet, gitGraph,
  architecture, radar, treemap, info, wardley, railroad*, eventmodeling, treeView, cynefin, usecase…),
  `structuredClone` (pie, radar, usecase dbs; mermaid config), `URL` (only `@braintree/sanitize-url` on
  `click X "http…"`; the polyfill's `URL.canParse` answers false, the href becomes `about:blank`, the parse
  answer is unchanged, corpus case `edge/click-callback`). Not needed: `window`, `document`, `console`, `atob`,
  `crypto`, `performance`, `queueMicrotask` (V8 has it), timers (referenced, never called on the parse path; the
  polyfill installs deno_core timers anyway). `Intl` is V8's ICU.
- **Size:** 5,368,716 bytes untrimmed; **2,860,387 bytes trimmed** (761 KB gzip). Trimming aliases render-only
  libraries to `src/inproc/stub.ts` (a Proxy whose every use throws, except `cytoscape.use(plugin)`
  registration at chunk load, a no-op): elkjs (3.29 MB unminified), cytoscape (1.03 MB), cytoscape-fcose, -cose-
  bilkent, katex (0.50 MB), roughjs, @upsetjs/venn.js. The trimmed bundle matches the corpus exactly like the
  untrimmed one (table above), so it is the default (`--no-trim` keeps them). d3 stays: several dbs and
  config code import d3 helpers at chunk load; not worth the risk for the remaining bytes. A first trim attempt
  that made `cytoscape.use` throw broke architecture's chunk load — caught by the corpus (3 mismatches), fixed
  by the `use` exception: the stub's throw-on-use is what makes trimming safe to verify.
- Load (evaluate the whole bundle): 45 ms in bun vm (JSC) trimmed / 75 ms untrimmed; 62 ms for the full
  deno_core runtime build (isolate + polyfill + bundle) in V8.

## 4. deno_core runtime: `crates/app-mermaid-js-spike`

Own copy of the generic runtime from `crates/document-js` on `feat/documents-inproc` **@ 2a806c37b** (the tip
when copied; includes the `init_platform` → `JsRuntime::init_platform(None)` fix, reply-to-request matching,
fatal "ended without its reply", and respawn of a dead thread). Same file/function layout so the two can be
unified into one generic crate later: `runtime.rs` (`MermaidJs` ≈ `DocumentsJs`: ops `op_mermaid_take_request`,
`op_mermaid_reply`, `op_mermaid_random_fill`, panic guard, heap-limit callback with 64 MB headroom, watchdog,
pump that drives the event loop until the reply op fires — `mermaid.parse` is a Promise), `thread.rs`
(`MermaidThread` ≈ `DocumentsThread`), `watchdog.rs` (verbatim), `js/polyfill.js`, `js/theuth-mermaid-inproc.js`
(embedded with `include_str!`). Differences, all simplifications: one reply line per request (no `ev:update`);
no epochs (mermaid holds no per-document state, so nothing must be reopened after a rebuild; instead, after a
fatal failure the queue is drained *before* the failing job's reply is sent, which gives the same guarantee —
a request sent after seeing a failure runs on the fresh runtime); the reply's `id` is checked in Rust against
the request's (string id, or `null` for a malformed line) and a mismatch is fatal; two deadlines
(`first_deadline` for the runtime build and the first request of each runtime, `deadline` for warm requests).

One V8: `deno_core = "0.412"` resolves to the same `deno_core 0.412.0` / `v8 150.4.0` as obscura-js
(`cargo tree -i v8 -p app-mermaid-js-spike`: §8).

### Timings and memory (Rust debug build; V8 is the prebuilt release static library; M-series laptop, other builds running)

| what | value |
|---|---|
| `init_platform` (direct `JsRuntime::init_platform(None)`) | 1.4 ms |
| runtime build: isolate + polyfill + bundle eval | 62 ms |
| first parse per type, one runtime, corpus order | 2–25 ms (pie 25, usecase 24, wardley 9, gitGraph 9, flowchart 8; most 2–6) |
| first parse per type on a **fresh runtime each** (worst case after a rebuild) | ≤ 35 ms (wardley 35, gitGraph 33, architecture 32, radar 31) |
| warm parse, 780 calls (every valid source ×20) | p50 1.6 ms, p90 2.9 ms, p99 5.3 ms, max 6.1 ms |
| warm 32 KB flowchart (under the edge limit) | 135 ms |
| process RSS: before / after bundle / after every type parsed / after warm | 3.4 MB / 65 MB / 113 MB / 117 MB |
| V8 heap used (total): loaded / after corpus / end | 11 MB (17) / 31 MB (54) / 33 MB (58) |
| current helper's jsdom path under bun (for comparison) | load jsdom + mermaid + first parse 183–439 ms; warm flowchart p50 0.8 ms; RSS 155 MB |

Warm per-call cost is ~2× the bun/JSC path for tiny diagrams (1.6 vs 0.8 ms p50; JSON + op round trip + V8 vs
JSC) and irrelevant at reply-fence frequency. The cold path is 3–7× faster than the helper's
(no jsdom, no process spawn).

### Recommended limits

- **First-call deadline: 10 s** (`FIRST_DEADLINE`), for the runtime build and the first request of each
  runtime. Measured worst is 62 + 35 ms; the old 15 s existed for jsdom's multi-second load in a child, which
  is gone. 10 s keeps ~100× headroom for a loaded machine without letting a wedged load hang a reply.
- **Warm deadline: 5 s for mermaid** (the helper's `DEADLINE` is 2 s; the crate default `DEFAULT_DEADLINE` is
  still 2 s as spiked). Measured under V8 (`tests::slow_fence_sized_inputs_unbounded`): class diagram with N
  `Cn <|-- Cn+1` lines: 500 → 0.98 s, 1000 → 1.60 s, 1500 → 2.36 s, 1900 (33.9 KB) → 3.01 s; 2417 plain class
  declarations (32 KB) → 1.37 s. bun/JSC (the current helper): 1000 → 0.93 s, 2000 → 1.87 s. So at 2 s V8 times
  out legitimate class diagrams from ~1300 lines (~22 KB); 5 s covers the 32 KB fence cap with margin. It only
  delays other mermaid checks (own thread, §7). Mermaid fences fail open: a timeout means "not checked".
- **Heap limit: 256 MB** (`DEFAULT_HEAP_LIMIT`). Steady state after every diagram type is 33 MB used / 58 MB
  total; the largest hostile input that ran to completion (3 MB label) stayed far below. 256 MB is ~4× the
  steady total and terminates a runaway (§5) in ~140 ms at 128 MB.
- **Max line: 4 MiB** (the helper's `MAX_LINE`). The fence cap is 32 KB, so this only bounds abuse.

## 5. Containment

Run in the same test binary; every case is followed by a normal parse on the same handle, which must succeed.

| input | result |
|---|---|
| `for(;;){}` (raw script, 500 ms deadline) | `Timeout` after 503 ms; next parse OK on a rebuilt runtime |
| heap bomb (`a.push(new Array(1e5).fill(...))`, 128 MB limit) | `OutOfMemory` after 138 / 182 ms (two runs); next parse OK |
| panic inside an op / on the runtime thread | `Crashed`; next parse OK |
| bundle answers another id / never answers | `Crashed` (fatal); next parse OK on a rebuilt runtime |
| runtime thread exits | next request respawns it and is answered |
| line over 4 MiB | `TooLarge`, never reaches JS |
| `%%{init: [[[…200k…]]]}%%` | `ok:false` "Maximum call stack size exceeded" in 23 ms (V8 RangeError, caught by mermaid's path; no abort) |
| front matter nested 100k | `ok:false` js-yaml "nesting exceeded maxDepth (100)" in 2 ms |
| `flowchart LR A((((…1M` | `ok:false` parse error in 27 ms |
| 3 MB label | `ok:true` in 63 ms |
| 3000 nested subgraphs | `ok:true` in 882 ms |
| gantt 20k `after` chain | `ok:true` in 82 ms |
| sequence 100k messages (1.5 MB) | `Timeout` at 2.0 s (bun/JSC: 9.9 s to finish); next parse OK |
| class inheritance 30k lines (608 KB) | `Timeout` at 2.0 s (bun/JSC: 48 s to finish); next parse OK |
| pie + 1M `"` (1 MB) | `Timeout` at 2.0 s (bun/JSC: > 2.5 min); next parse OK |
| class inheritance, 32 KB (fence-sized) | `Timeout` at 2.0 s (3.0 s unbounded, see §4); next parse OK |
| pie + 32K `"` (fence-sized) | `ok:false` parse error in 540 ms |
| sequence, 32 KB (fence-sized) | `ok:true` in 402 ms |
| mindmap depth 3000 (4.5 MB) / 3M blank lines (6 MB as JSON) | `TooLarge` (over 4 MiB), never reaches JS |

Five runtimes were built in the hostile run (four Timeouts → four rebuilds); each rebuild is ~60 ms and lazy.

**No input aborted the process.** Super-linear mermaid parsers found while probing (bun vm, JSC, i.e. the same
behaviour the current helper has): class diagram ~0.9 ms per `C1 <|-- C2` line (1000 lines 0.93 s, 2000 lines
1.87 s, 30k lines 48 s); pie with N unterminated `"` grows quadratically (16k: 0.2 s; 1M: > 2.5 min);
sequence ~0.1 ms per message (100k: 9.9 s); 3M blank lines before a flowchart > 4 min. The watchdog
terminates all of them at the deadline (V8 `TerminateExecution` interrupts JIT code and regex backtracking).

## 6. Risks

- **Loss of process isolation.** Same list as documents-inproc P3: a V8 internal fatal error, a single
  allocation larger than the 64 MB heap headroom, exhaustion of memory outside the V8 heap (ArrayBuffer
  backing stores), or a panic inside a deno_core builtin op aborts `t`. Mermaid's parse path allocates only
  ordinary heap objects; nothing in the corpus or hostile set came close.
- **Slow legitimate inputs.** 32 KB class diagrams take 3.0 s under V8 (1.9 s under the current helper's JSC).
  With the 2 s deadline the in-process checker would leave more large class diagrams unchecked than today.
  Give mermaid its own 5 s warm deadline (it is on its own thread, so this blocks only other mermaid checks),
  or cap mermaid fences lower than 32 KB. Recommendation: 5 s.
- **Polyfill drift.** A mermaid upgrade can start using another web global on the parse path. The corpus run
  in the bare context catches that as a mismatch (that is how the three were found); keep it as a test.
- **Identity DOMPurify.** Safe only because nothing renders; the shim must never be used by a renderer (it is
  only reachable through `build-inproc-mermaid.ts`).
- **Engine-worded messages.** TypeErrors from inside mermaid carry engine text; already unstable today.

## 7. Integration (production)

**Plugging into the sibling's seam** (branch `feat/apps-inproc`, `crates/app-js`, `crates/app/src/helper.rs`
marked mermaid seam): a *second* isolate, `MermaidThread`, on its own lazily started `mermaid-js` thread,
separate from the `check` isolate, so a mermaid chunk load or a 2 s class diagram never delays `check`'s 2 s
deadline, and a mermaid runtime rebuild never drops the OpenUI runtime. `fence.rs` mermaid fences call
`MermaidThread::request(line)`; Timeout/OutOfMemory/Crashed map to the existing fail-open path in
`agent::loop_::check_fences`. The PENDING=8 admission bound stays in `helper.rs`.

Work left, estimated **6–8 hours**:

1. Move `runtime.rs`/`thread.rs`/`watchdog.rs`/`polyfill.js` into `crates/app-js` as a second runtime (or,
   better, factor the shared runtime with `document-js` once both are merged: the files were kept structurally
   identical for that). 2 h.
2. Golden tests as integration tests (`tests/golden.rs`, like document-js) over `golden.json`; the bun
   `parity-vm.ts` as a package test. 1 h.
3. `just apps-mermaid-bundle` / `apps-mermaid-bundle-check` with a `BUNDLE.sha256` over
   `packages/apps/src/{inproc-mermaid.ts,mermaid-core.ts,protocol.ts,inproc/**}`, `package.json`, `bun.lock`,
   `scripts/build-inproc-mermaid.ts` (same scheme as `documents-bundle`). 1 h.
4. Wire into the seam: `fence.rs` → `MermaidThread`; delete `MERMAID_FIRST_DEADLINE` and the
   `check_mermaid`-specific deadline code in `helper.rs`. 1–2 h.
5. Remove the jsdom path: `src/mermaid-check.ts` (keep `mermaid-core.ts`), `warmMermaid`/`check_mermaid` in
   `helper.ts` and `protocol.ts` if the helper binary is gone entirely (sibling's call), `jsdom` from
   `package.json` (still used by the React tests via `test/dom.ts`, so it moves to devDependencies rather than
   going away), design 46 §2.1 and 30 §5 notes. 1–2 h.
6. Set the mermaid warm deadline to 5 s (§4). Trivial.

## 8. Commands and outputs

All run on this branch; the Rust side is a debug build.

```
cd packages/apps
bun run test-inproc/mermaid-corpus/reference.ts          # 148 cases → golden.json (jsdom path)
bun run scripts/build-inproc-mermaid.ts --out ../../crates/app-mermaid-js-spike/js/theuth-mermaid-inproc.js
bun run test-inproc/mermaid-corpus/parity-vm.ts ../../crates/app-mermaid-js-spike/js/theuth-mermaid-inproc.js
  → exact 147/148, ENGINE-WORDING edge/timeline-typeerror

CARGO_TARGET_DIR=$PWD/target CARGO_BUILD_JOBS=4 cargo test -p app-mermaid-js-spike --no-run --offline
target/debug/deps/app_mermaid_js_spike-<hash> --test-threads=1      (from crates/app-mermaid-js-spike)
  → test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 26.86s
target/debug/deps/app_mermaid_js_spike-<hash> corpus_matches_golden_and_measure --nocapture
  → PARITY exact 147/148
    PARITY engine-wording edge/timeline-typeerror: … v8 "Cannot read properties of undefined (reading 'events')"

CARGO_TARGET_DIR=$PWD/target CARGO_BUILD_JOBS=4 cargo tree -i v8 -p app-mermaid-js-spike --offline
v8 v150.4.0
└── deno_v8 v0.4.0
    ├── deno_core v0.412.0
    │   └── app-mermaid-js-spike v2.22.2 (…/crates/app-mermaid-js-spike)
    └── serde_v8 v0.321.0
        └── deno_core v0.412.0 (*)
```

Cargo.lock holds one `v8` (150.4.0) and one `deno_core` (0.412.0), the versions obscura-js already pulls.
