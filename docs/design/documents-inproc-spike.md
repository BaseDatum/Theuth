# Documents helper in-process (route A): feasibility spike

Branch `spike/documents-inproc` (cut from main 729093ffc). Question: can the documents helper (`packages/documents`: `convert.ts` + BlockNote 0.54.2 + yjs), today a 69 MB `bun build --compile` child spoken to over stdio (59 §0 decision 1, §3.2, §24.7), run unchanged in a bare `deno_core` `JsRuntime` inside `t`, with the same wire and the same results?

## Verdict

**Works, with caveats.** The helper's converters run without jsdom and React on linkedom + 3 small patches (same 83/84 suite result as the jsdom baseline), as one 1.1 MB runtime-neutral bundle, in a bare deno_core 0.412 `JsRuntime` (the V8 `t` already links through obscura-js) with a 78-line JS polyfill over three ops: **52/52 parity inputs match the compiled 69 MB helper** (markdown byte-for-byte, replies/outline with ids normalised, decoded Yjs block JSON). Cold start to first successful import is 77–80 ms vs 158 ms for the child; `markdown` on a 48 KB document is ~2.3× faster, `import` diff ~1.2× faster, but `import` replace ~1.4× slower (V8 vs JSC on this workload). Caveats: loss of process isolation (a V8 fatal OOM aborts `t`), owning the linkedom patches and a 67-line `ServerBlockNoteEditor` drop-in, and the `import` regression (§5, §8).

## 1. The DOM: what the helper's paths need, and the swap

`ServerBlockNoteEditor` (server-util 0.54.2) is a thin wrapper. The helper calls exactly four of its methods; what each needs:

| call (`convert.ts`) | core function | DOM use |
|---|---|---|
| `yXmlFragmentToBlocks` | `@blocknote/core/yjs` → y-prosemirror → `nodeToBlock` | none |
| `blocksToYXmlFragment` | `@blocknote/core/yjs` → `blocksToProsemirrorNode` → y-prosemirror | none |
| `tryParseMarkdownToBlocks` | `markdownToHtml` (pure string parser, no unified/remark) → `HTMLToBlocks` | `document.implementation.createHTMLDocument`, `innerHTML=`, `querySelectorAll("li > ul, li + ol")`, `createElement`, `createTreeWalker(root, SHOW_TEXT/SHOW_COMMENT, filter)`, ProseMirror `DOMParser.parse` (childNodes, nodeName, getAttribute, `matches`, style), `HTMLInputElement.checked` |
| `blocksToMarkdownLossy` | `createExternalHTMLExporter` → ProseMirror `DOMSerializer` + block `toExternalHTML` renders → `htmlToMarkdown` (DOM walk, no turndown) | `createElement`, `createTextNode`, fragments, `setAttribute`, `innerHTML` get/set, `querySelector(All)` (`code`, `tr`, `th, td`, `colgroup`, `summary`), `textContent`, `tagName`, `checked` |

Editor construction (`BlockNoteEditor.create`) runs outside any DOM in server-util already and needs none.

**React is not needed.** server-util imports React/react-dom only for `withReactContext` and `BlockNoteViewRaw`; none of the four calls touches them. The spike's drop-in (`packages/documents/src/inproc/editor.ts`, same class name and the four methods, built on `@blocknote/core` directly) has no React import, and the bundle check (`scripts/build-inproc.ts`) fails if `react`, `react-dom`, `@blocknote/react`, `jsdom` or `@blocknote/server-util` appear.

`convert.ts` is unchanged: the drop-in replaces `@blocknote/server-util` by module aliasing (`mock.module` in `test-inproc/preload.ts` for `bun test`; an `onResolve` plugin in `scripts/build-inproc.ts` for the bundle). The DOM is swapped in exactly as server-util swaps jsdom (`globalThis.document/window` set only while a conversion runs).

### Suite results per DOM (unchanged `test/*.test.ts`: 84 test cases)

Why 84 and not 68: `test/*.test.ts` has 68 `test()` call sites (convert 24, edit 31, helper 6, yaml 7); one of them is the `markdown round trip per block type` table, a loop that registers 17 cases, so `bun test` runs 68 − 1 + 17 = 84. Re-run on 2026-11-21 for this report: jsdom baseline `bun test test/` → 83 pass, 1 fail; `DOM=linkedom bun test --preload ./test-inproc/preload.ts test/` → 83 pass, 1 fail, the same test.

The failing test in both: `markdown round trip per block type > code: link`. Input `See [parse()](code:crates/surface/src/lib.rs#L71-L84) and [x](theuth://notes/1).`; the test expects the `theuth://` link stripped to `x`, the helper keeps it. `src/schema.ts` deliberately allowlists the `theuth:` scheme (144 §6: note citations must survive import/paste/export), so the test's expectation is stale; it fails identically on main and is not a DOM or runtime issue.

| DOM | pass | fail | notes |
|---|---|---|---|
| jsdom via server-util (baseline, `bun test`) | 83 | 1 | the 1 is pre-existing on main: `markdown round trip per block type > code: link` expects `[x](theuth://notes/1)` to be stripped, but `schema.ts` allowlists `theuth:` since design 144 §6 — a stale test, not a DOM issue |
| linkedom 0.18.15, unpatched | 15 | 69 | no `document.implementation` → every markdown import throws |
| linkedom + `implementation.createHTMLDocument` | 79 | 5 | code blocks lose newlines (TreeWalker ignores `filter`), `- [x]` imports unchecked (no `HTMLInputElement.checked`) |
| **linkedom + 3 patches (`src/inproc/dom.ts`)** | **83** | **1** | same single stale failure as baseline |
| happy-dom 21.0.4, unpatched | 83 | 1 | same single failure; but imports `node:` built-ins (net, url, vm, …) and `ws`, so it cannot go into a runtime-neutral bundle |

Run: `DOM=linkedom bun test --preload ./test-inproc/preload.ts test/` (or `DOM=happy-dom`).

**Chosen: linkedom** + three local patches in `src/inproc/dom.ts`:

1. `document.implementation.createHTMLDocument()` → a fresh linkedom document (nestedLists.ts' detached parse root).
2. `createTreeWalker(root, whatToShow, filter)` honours `filter` (linkedom drops it; BlockNote's whitespace normaliser relies on `FILTER_REJECT` inside `<pre>`/`<code>`).
3. `HTMLInputElement.prototype.checked` reflected from the `checked` attribute (CheckListItem parse/render).

linkedom is pure JS (htmlparser2, css-select, entities, cssom-lite), no Node built-ins. Why not obscura's DOM: see §1.1.

### 1.1 Could obscura's own DOM replace linkedom? (read, not run)

From reading obscura at the pinned checkout (ee30f89):

- `obscura-dom` is a Rust tree on html5ever/markup5ever + Servo `selectors` (`parse_html`, `parse_fragment`, `DomTree`); no JS dependency. The Web DOM classes are JS in `obscura-js/js/bootstrap.js` (~19k lines) over one string-command op, `op_dom(cmd, a1, a2, frame_id)`. It has what the 3 patches add to linkedom: `DOMImplementation.createHTMLDocument` (bootstrap.js ~6366), `createTreeWalker(root, whatToShow, filter)` (~6781), `HTMLInputElement.checked`.
- It cannot be loaded into a fresh `JsRuntime`: the ops are private (only `build_extension()` is public), every op expects `Rc<RefCell<ObscuraState>>` (cookie jar, HTTP clients, storage, navigation, CDP queues) and `RealmStates` in `OpState`, the classes live only in obscura's build-time startup snapshot, and `ObscuraJsRuntime::new()` removes `Deno.core` from the global, so no spike ops could sit beside it.
- The supported way is to host the bundle in an `ObscuraJsRuntime` page (`new` → `set_dom(parse_html(…))` → `run_page_init` → `execute_script`), i.e. a full browser-shaped global per runtime, every DOM call crossing into Rust through `op_dom` (BlockNote's import/export touch many small nodes; likely slower than linkedom's in-heap objects — unmeasured).

Verdict: possible but heavier, more coupled and unproven; linkedom (+3 patches, 0 Rust) is the boring choice. Not run.

## 2. Runtime-neutral bundle

`bun run scripts/build-inproc.ts` → `packages/documents/dist/theuth-documents-inproc.js`: one minified IIFE, `--target=browser`, entry `src/inproc.ts` (transport-agnostic copy of `helper.ts`'s `parseRequest`/`handle`, pure-JS base64, events collected per call instead of written to stdout).

- Size: **1,104,902 bytes** minified (335,808 gzip). Compare: 69,360,114-byte compiled helper.
- 41 packages: @blocknote/core, @tiptap/* (core + 7 extensions), prosemirror-* (11), y-prosemirror, yjs, lib0, linkedom + htmlparser2/css-select/entities/domhandler/…, fast-deep-equal. No React, no jsdom, no `node:`/`bun:` specifiers, no `require(` (the build fails otherwise).

Globals it expects (measured: bundle run in an empty `node:vm` context with a recording global, then the full parity script):

| global | when | needed? |
|---|---|---|
| `crypto.getRandomValues` | load (lib0 binds it) + every Y.Doc / block id | **required** |
| `crypto.randomUUID` | `convert.ts` empty paragraph / frontmatter ids | **required** |
| `crypto.subtle` | read once at load by lib0's webcrypto module, never used | property read only (undefined is fine) |
| `atob` | load: htmlparser2/entities decode their tables | **required** |
| `clearTimeout`, `clearInterval` | load: lib0 eventloop binds them as values | **required as identifiers**; never called on these paths |
| `setTimeout`, `setInterval` | only ProseMirror view / BlockNote UI plugins (never instantiated headless) | provided for safety |
| `TextEncoder`, `TextDecoder` | `typeof`-probed by lib0; pure-JS UTF-8 fallback when absent | optional (perf) |
| `process`, `navigator`, `location`, `window`, `document`, `DOMParser` | `typeof`-probed by libraries at load | absent is correct (`document`/`window` are set by the drop-in during conversions) |

Not touched at all: `structuredClone`, `URL`, `performance`, `queueMicrotask` (Promises use V8's own microtask queue), `fetch`, `Buffer`.

Same-engine check before deno_core: the bundle served over stdio from a bare `node:vm` context with only `crypto`, `atob` and the four timer functions (`test-inproc/serve-bundle.ts`) matched the compiled jsdom helper on **52/52** parity inputs (§4).

## 3. deno_core

**One V8.** `crates/document-js-spike` depends on `deno_core = "0.412"` with default features, exactly as `obscura-js` does (its `Cargo.toml`: `deno_core = "0.412"`), and on `getrandom` 0.2, `serde_json`, `tokio` (all already in `Cargo.lock`). `cargo tree --offline -i v8@150.4.0 -e normal`: one `v8 v150.4.0` ← `deno_v8 v0.4.0` ← `deno_core v0.412.0` ← {`document-js-spike`, `obscura-js`}. `Cargo.lock` gains only the spike's own entry (+10 lines). The `theuth` binary already links this V8 unconditionally (`crates/theuth` → `browser` → `obscura-js`, no feature gate).

**Built and run.** `cargo build --offline -p document-js-spike` (dev profile, under the build lock, 17:00): 2.4 s for the crate itself once dependencies were built; `target/debug/document-js-spike` is 82,252,672 bytes (debug, line tables, includes V8). `document-js-spike globals`:

```
{"console":"object","queueMicrotask":"function","setTimeout":"function","clearTimeout":"function","setInterval":"function","clearInterval":"function","crypto":"object","atob":"function","btoa":"function","TextEncoder":"undefined","TextDecoder":"undefined","structuredClone":"undefined","URL":"undefined","performance":"undefined","window":"undefined","document":"undefined","navigator":"undefined","process":"undefined","Buffer":"undefined","theuthDocuments":"object"}
```

A bare deno_core 0.412 runtime provides only `console` (over `op_print`), `queueMicrotask`, `Deno.core` (with `createTimer`/`cancelTimer`, `encode`/`decode`, `structuredClone` reachable there) and ECMAScript built-ins (Intl with ICU from the v8 crate); everything else above comes from the spike's polyfill.

**How it runs** (`src/lib.rs`): `DocumentsJs::new` builds a current-thread tokio runtime, creates the `JsRuntime` inside it (deno_core binds the isolate to the tokio handle current at construction), with one hand-built `Extension` of three ops (obscura's pattern), then runs `js/polyfill.js`, the bundle (`include_str!` of `packages/documents/dist/theuth-documents-inproc.js`) and a 6-line pump as classic scripts. `call(line)` puts the request into `OpState`, runs `__theuthPump()` (calls `theuthDocuments.call(line)`, hands the resolved output to `op_spike_reply`), drives `run_event_loop` to idle and returns the output lines. No module loader, no snapshot, no `Deno` namespace beyond `Deno.core`.

Polyfills/ops (everything the vm probe in §2 found; nothing else was needed for 52/52 parity):

| global | provided by |
|---|---|
| `crypto.getRandomValues` | JS wrapper over `op_spike_random_fill` (`#[op2(fast)]`, `#[buffer] &mut [u8]`, `getrandom` 0.2 = OS randomness) |
| `crypto.randomUUID` | JS, v4 UUID from `getRandomValues` |
| `atob` (and `btoa`) | pure JS |
| `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval` | JS over `Deno.core.createTimer`/`cancelTimer` (`02_timers.js`: `createTimer(callback, after, args, isRepeat, isRefed, isSystem)`) |
| request/reply transport | `op_spike_take_request` (`#[op2] #[string] -> String`), `op_spike_reply` (`#[op2(fast)]`, `#[string] String` — deno_ops 0.288 rejects a fast-compatible signature without `(fast)`) |
| `queueMicrotask`, `console` | deno_core's own bootstrap |

`TextEncoder`/`TextDecoder` are deliberately absent (lib0 falls back to its pure-JS UTF-8 path, as in the vm parity run); adding them over `Deno.core.encode`/`decode` is a possible speed-up, not measured.

**Snapshot: not needed.** Bundle evaluation is ~67–69 ms once per process (on the documents thread, lazily); a startup snapshot would save that at the cost of a build-time snapshot step (obscura does this in its `build.rs`; deno_core asserts a process never mixes snapshot-creating and normal runtimes, so it must be build-time).

Added binary size over a deno_core-only baseline: **not measured** (would need a second build). [INFERENCE] For `t` the delta is the 1.1 MB bundle plus a few KB of op glue: V8 and deno_core are already linked.

## 4. Parity

Driver: `scripts/parity-inproc.ts <helper-bin> <peer-cmd>` — 52 inputs: 15 `test:` (the `convert.test.ts` round-trip inputs plus the frontmatter and outline goldens), 13 `syn:` edge cases (deep nesting, marks, link schemes incl. `javascript:`, quarto fences, json-render, table marks, unicode, HTML-ish, frontmatter-only, empty, mixed fences, numbered starts, nested quotes), 10 sections of design doc 59, and 14 notes sampled from `~/.theuth/knowledge`. Per input both peers get the same script (open; import replace; markdown; outline; annotated markdown; edit insert/replace/move/delete; markdown; undo edit; markdown; import diff; outline depth 2; markdown of selected blocks; an edit on a missing block (error path); import append; markdown; close), and the driver compares every reply line with block ids normalised (first-appearance order per side), the `ev:update` metadata, markdown byte-for-byte, and the decoded block JSON of an authority `Y.Doc` fed each side's updates.

| peer B (A = compiled helper, jsdom + Bun) | inputs | match | mismatch |
|---|---|---|---|
| bundle in a bare `node:vm` context under Bun (`test-inproc/serve-bundle.ts`; linkedom, JSC) | 52 | 52 | 0 (re-run 2026-11-21: `bun run scripts/parity-inproc.ts dist/theuth-documents "bun test-inproc/serve-bundle.ts"` → `52 inputs, 52 match, 0 mismatch`) |
| bundle in deno_core (`document-js-spike serve`; linkedom, V8) | 52 | 52 | 0 (17:01: `bun run scripts/parity-inproc.ts dist/theuth-documents "../../target/debug/document-js-spike serve" --report /tmp/parity-deno.json` → `52 inputs, 52 match, 0 mismatch`) |

The bare-vm row separates DOM differences from engine differences; the deno_core row adds the engine change (JSC → V8) and the real polyfills, and still matches on every input.

## 5. Measurements

Bundle: **1,104,902 bytes** minified, 335,808 gzip (vs the 69,360,114-byte compiled helper).

All on the shared machine (load average 17–20 at the time, so medians are noisy), `/tmp/doc50k.md` = the first 48,642 bytes of design doc 59 cut at a section boundary (→ 49,435 bytes of markdown out). Driven by the crate's own binary: `document-js-spike bench /tmp/doc50k.md N` (in-process, `DocumentsJs::call` directly) and `document-js-spike bench-stdio <peer> /tmp/doc50k.md N` (same request script through a child's stdio). Debug Rust; V8 is the prebuilt release library, so JS timings are representative. Medians (min–max).

| | cold start → first successful `import` | `import` replace | `markdown` | `outline` | `import` diff |
|---|---|---|---|---|---|
| **deno_core in-process** (linkedom, V8), run A, n=20 | **77.2 ms** (`JsRuntime::new` 4.5 incl. V8 platform init + polyfill/bundle eval 66.8) | 47.3 ms (38.5–71.5) | **11.1 ms** (8.9–25.6) | 2.3 ms | **70.0 ms** (55.6–85.8) |
| deno_core in-process, run B, n=20 | — | 44.3 ms (30.7–73.3) | 11.7 ms (8.9–29.9) | 2.3 ms | 63.9 ms (47.0–92.5) |
| deno_core in-process, run C (concurrent session), n=10 | 80.0 ms (5.0 + 68.9) | 48.8 ms (30.8–70.8) | 10.2 ms (9.1–19.3) | 2.3 ms | 69.7 ms (47.2–77.2) |
| deno_core in-process, run D (concurrent session, 17:05, reproduction), n=20 | 80.3 ms (4.9 + 68.8) | 44.2 ms (37.9–72.5) | 9.1 ms (8.1–54.4) | 2.1 ms | 63.2 ms (52.0–86.6) |
| compiled helper over stdio, run D (17:05), n=20 | — | 33.1 ms (29.8–79.3) | 25.6 ms (21.6–44.4) | 3.9 ms | 82.3 ms (75.4–106.7) |
| **compiled helper over stdio** (jsdom, Bun), n=20 | 157.9 ms (spawn included) | **33.5 ms** (30.6–79.2) | 25.1 ms (22.6–44.4) | 2.1 ms | 83.2 ms (79.1–102.2) |
| bundle under Bun in a bare `node:vm` over stdio (linkedom, JSC), n=20 | (299.6 ms: `bun` startup + vm) | 31.4 ms (24.7–55.3) | 7.3 ms (6.3–17.1) | 2.1 ms | 42.1 ms (37.9–59.4) |
| deno_core runtime behind stdio (`serve`), n=20 | (331.7 ms: 82 MB debug binary + thread) | 50.4 ms (32.1–80.2) | 11.3 ms (9.7–24.3) | 2.5 ms | 72.6 ms (49.7–93.8) |

Reading:

- Cold start halves (77–80 ms vs 158 ms) and needs no process; a second runtime in the same process costs 60–62 ms.
- `markdown` is ~2.3× faster in-process: the DOM swap (jsdom → linkedom) is the win; the Bun/linkedom row shows the same DOM on JSC is faster still.
- `import` replace is ~1.4× **slower** in-process. Same bundle on JSC (31 ms) vs V8 (44–50 ms, also when served over stdio), so it is the engine on this workload, not the DOM, the ops or the in-process plumbing. Not investigated further (candidates: lib0's non-`TextEncoder` path, GC on the busy machine, ProseMirror/y-prosemirror tiering).
- `import` diff is ~1.2–1.3× faster in-process (it is dominated by per-block `blocksToMarkdownLossy`, i.e. the DOM).
- The stdio round trip itself is small: in-process vs the same runtime behind `serve` differ by ≤3 ms per op.
- Memory (RSS of the isolate vs the 69 MB child): **not measured**.

An earlier pass with a throwaway Bun driver (concurrent session, ~16:25, n=10) put the compiled helper at 253.8 ms cold, 38.5 ms import, 32.2 ms markdown and the Bun/linkedom bundle at 86.9 ms / 32.5 ms / 9.1 ms — same ordering.

## 6. Threading model and how it replaces `Helper`

`JsRuntime` is `!Send` and binds to the tokio handle current at construction, so it cannot live inside `Helper`'s tokio worker task. Proposal, prototyped as `DocumentsThread` in `crates/document-js-spike/src/lib.rs`:

- One dedicated OS thread (`documents-js`, 16 MB stack) owns a current-thread tokio runtime and the `JsRuntime`; it is started lazily on the first request (keeps `t`'s startup and idle RSS unchanged when no document is open).
- `Helper` keeps its public shape: `Clone` handle holding an `mpsc::UnboundedSender<Job>` and the `AtomicU64` id counter. `request` builds the same JSON line as today and sends a `Job { id, line, reply: oneshot::Sender }`; the thread runs `call(line)`, which returns the helper's output lines for that request (`ev:update` lines, then the reply). The thread forwards each `ev:update` to `events` as `HelperEvent::Update` (the existing `handle_line` parser, unchanged) and resolves the `oneshot` with the reply. `Pending`, `open/update/close/markdown/markdown_of/import/edit/outline`, `import_result`/`edit_result`/`log` are untouched: they only ever saw `Pending` and `HelperEvent`.
- Ordering: one queue, one thread, requests run to completion in enqueue order — the same guarantee the stdio pipe gives today (`open` enqueued synchronously before `update`).
- Failure: a JS exception becomes `{"ok":false,"error":…}` (already how the wire reports errors). A V8 OOM or panic on the thread is the in-process analogue of a child crash: catch the panic at the thread boundary, fail all pending with `Error::Crashed`, build a fresh runtime, send `HelperEvent::Restarted` (mirrors gone, reopened by the hub exactly as now). A V8 fatal OOM aborts the whole process instead — see §8.
- Shutdown: dropping the last sender ends the thread's loop; `shutdown: CancellationToken` closes the channel instead of killing a child.
- Deadline: today `DEADLINE` (30 s) kills a hung child. In-process, a watchdog holding the isolate's thread-safe `IsolateHandle` calls `terminate_execution()` when a request passes the deadline (obscura-js does the same in `cdp_watchdog.rs`); the thread then drops and rebuilds the runtime and sends `Restarted`.
- V8 platform: `t` already initialises V8 once on the main thread (`browser::warmup` before any plugin thread); the documents thread only creates an isolate afterwards. Tests that build `DocumentsJs` without obscura initialise it themselves (deno_core's `init_v8` is behind a `Once`).
- Placement: putting the runtime into `crates/document` makes every `cargo test -p document` link V8. Cheaper for the dev loop: a small `document-js` crate (this spike, grown up) that `theuth` wires in, with `Helper` taking the backend at construction; `crates/document` tests keep a fake or the stdio helper behind the same channel.

## 7. What could be deleted

If the in-process runtime replaced the child (counted on main 729093ffc):

- `crates/document/src/helper.rs` (1,089 lines): `Proc`, `spawn` (`--version` probe, piping, stderr ring), the reader task and `MAX_LINE` line cap, `helper_path`, `protocol_version` as a file-name/handshake check, `HelperStatus`/`helper_status`/`HelperProcess`/`set_process` (the doctor's "helper found / version" report), the restart-on-EOF branch of `worker`, and the shell-script fake-helper tests (`script(...)`, `--version` stubs). Kept: `Helper`, `Pending`, `HelperEvent`, `Error` (minus `NotFound`), result types, `handle_line`. Callers that build `Helper::new(path, …)` (`hub.rs` tests, `crates/agent/src/rpc_documents.rs`, `crates/artifact/tests/{document,read}.rs`) lose the `path` argument.
- `justfile`: the `documents-helper` recipe and its use in `install` and `desktop`.
- `scripts/app-parser-env.sh`: `DOC_VERSION`/`DOC_HELPER`/`DOC_HELPER_SRC`/`build_doc_helper`; and the `theuth-documents-*` copy/sign/notarise/upload steps in `scripts/package-macos.sh`, `package-update-macos.sh`, `install-macos.sh`, `package-linux.sh`, `stage-linux.sh`, `package-windows.ps1`, `release-s3.sh`, `scripts/tap/theuth.rb.formula.tmpl` (38 matching lines across these files; the updater's `theuth-documents-[0-9.]+` acceptance too).
- `packages/documents`: `src/helper.ts` (stdio transport; `src/inproc.ts` is its replacement), `scripts/build-helper.ts`, the `@blocknote/server-util`, `jsdom` and React dependencies.
- Shipping: one 69 MB per-platform binary and its signing; `t` grows by the bundle (~1.1 MB, or ~336 KB if stored gzip and inflated at first use) and gains no new native code (V8 is already linked via obscura-js).

## 8. Risks

- **Process isolation is lost.** Today a runaway import (pathological markdown, deep nesting) or a BlockNote bug that OOMs kills only the child; in-process, V8's fatal OOM handler aborts `t`. Mitigation: `RuntimeOptions::create_params` with a heap limit plus `add_near_heap_limit_callback` to terminate execution and rebuild the runtime; not measured in this spike.
- **No preemption.** A long conversion blocks the `documents-js` thread (other document requests queue behind it, as with the single child today); `t`'s tokio workers are unaffected. A watchdog can call `IsolateHandle::terminate_execution` from another thread.
- **linkedom fidelity.** jsdom and linkedom differ; the 3 patches cover what the test suite and the parity corpus exercise. Other BlockNote paths (new block types, paste HTML in `edit`) may hit further linkedom gaps. Keep the parity script as a CI check against a jsdom run.
- **Drift of the patches**: `src/inproc/dom.ts` patches linkedom 0.18.15 prototypes; a linkedom upgrade can silently change them. Pin the version.
- **deno_core / V8 coupling**: the crate must track obscura-js's `deno_core` version so one V8 is linked; a deno_core bump in obscura forces re-checking the op attributes and `execute_script` signature here.
- **Memory**: each `JsRuntime` is a V8 isolate; one shared isolate for all documents is assumed (as one child today).
- **Upstream BlockNote**: replacing `ServerBlockNoteEditor` with the spike's drop-in (`src/inproc/editor.ts`) means owning ~the four methods' wiring on BlockNote upgrades.

## 9. Estimate for the real integration

About **14–20 hours** for the real integration, given this spike's code:

| step | h |
|---|---|
| move `DocumentsJs`/`DocumentsThread` into `crates/document` (feature-gated or default), bundle via `build.rs`/`include_str!` with a just recipe for `bun run scripts/build-inproc.ts` | 2–3 |
| rework `helper.rs` worker onto the thread (ev:update fan-out, restart on panic, heap-limit callback), keep `Helper`'s API; update the 5 call sites of `Helper::new` | 4–5 |
| make `src/inproc.ts` the only entry (fold `helper.ts`), drop server-util/jsdom/React deps; keep `bun test` running under linkedom by default | 2 |
| parity script → CI check (jsdom vs in-process) and the existing `crates/document`/`crates/artifact` tests against the in-process runtime | 2–3 |
| delete packaging/justfile/updater/doctor pieces (§7), docs | 2–3 |
| buffer for linkedom gaps found by the wider tests, and V8 heap-limit testing | 2–4 |

## Files (spike)

- `packages/documents/src/inproc/editor.ts` — `ServerBlockNoteEditor` drop-in (core functions, pluggable DOM, no React)
- `packages/documents/src/inproc/dom.ts` — linkedom + 3 patches; `dom-happy.ts` — happy-dom candidate (test-only)
- `packages/documents/src/inproc.ts` — runtime-neutral wire entry, `globalThis.theuthDocuments.call(line)`
- `packages/documents/scripts/build-inproc.ts` — the bundle + its checks
- `packages/documents/scripts/parity-inproc.ts` — parity driver (two stdio peers)
- `packages/documents/test-inproc/preload.ts`, `serve-bundle.ts` — suite under the swap; bundle in a bare vm
- `crates/document-js-spike/` — `DocumentsJs` (runtime + `call`), `DocumentsThread` (Send handle), `js/polyfill.js`, the `serve`/`globals`/`bench`/`bench-stdio` binary
