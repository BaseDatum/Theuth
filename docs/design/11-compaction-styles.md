# 11 — Compaction styles: operator-chosen cut, default algorithm

Status: as-built v0 · 2026-09-10 · extends `07-context.md` (epochs, cut,
cache-hot summariser, `/compact`), `02-models-and-menus.md` §4 (slash),
`01-config-and-subagents.md` §1.1 (config rewrite). Where this doc
disagrees with those, this doc wins; §9 lists the edits.

Goal: the operator can pick **how** a compaction folds history, while
auto-compaction and a bare `/compact` still run a single default algorithm.
The epoch machine in 07 does not change: one cut, one new head, messages
append-only.

oh-my-pi (`../misc/oh-my-pi/docs/compaction.md`) is the inspiration for
*named methods the operator can force*. It is not the architecture. Its
default is a five-method fallback list (`remote`, `snapcompact`, `handoff`,
`shake`, `soft`) plus pruning, idle compaction, speculative compaction, and
provider-native `/responses/compact`. 07 §14 already rejected the pieces of
that list that rewrite history in place or add a utility model. This doc
takes three styles that fit the epoch invariant, one config key, and a
slash surface.

## 0. Decisions in one screen

1. **Three styles.** `summary` (today's cache-hot LLM fold — the default),
   `cut` (mechanical: keep the tail, no model call), `snap` (mechanical:
   rasterize the folded span onto PNG frames in the new head, no model
   call). Nothing else in v0.
2. **One default, not a method order.** `agent.compact_style` (default
   `summary`) is what auto-compaction, overflow recovery, and a bare
   `/compact` run. A `/compact <style>` is a one-shot override. There is
   no `methodOrder` list to walk.
3. **Fallback is two lines, not a chain.** Snap on a text-only model, or a
   summariser that itself overflows, becomes `cut`. That is the whole
   default algorithm. Failed manual snap does not silently summarise.
4. **`/compact` grows arguments; it does not become a menu.**
   `/compact` runs the default; `/compact summary|cut|snap` runs that
   style once; `/compact style` lists styles and the default;
   `/compact style <name>` rewrites `agent.compact_style` in `config.yaml`
   the way `/model` rewrites `agent.model`.
5. **The cut is still 07 §8.2.** Styles differ only in how the *head* is
   built. `keep_from`, the epochs row, harvest-before-cut (10 §5.2),
   harness lines on the head (roster / todo / runtime / knowledge), and
   the cache-miss log line stay.
6. **Heads declare their style.**
   `<context_summary epoch="n" style="cut">`. Existing heads with no
   `style` attribute are `summary`. The store gains no column.
7. **Snap is a style in v0's contract, a later crate in the build.** The
   operator-facing name and the head shape are specified here. The
   rasterizer (`agent::snap`) lands after `summary`/`cut` compile. A
   `/compact snap` before that is `Err("snap not built")`. Build plan,
   budget and geometry: `13-snap.md` (which wins where it differs).

## 1. What oh-my-pi actually does

The user's gloss — "snap-compact, where things are just removed from
context without actually generating a summary" — is the *intent* we want
(`cut`). It is not what oh-my-pi's `snapcompact` method does.

| omp method | what it does | maps to |
|---|---|---|
| `soft` | LLM summary (often a cold utility-model call over a re-serialised transcript) | `summary`, except we keep 07 §8.3: cache-hot continuation of the live frame, same session model, `COMPACT_INSTRUCTION` |
| `snapcompact` | local, no LLM: serialize the discarded span, print it on model-aware PNG frames, put those images in context | `snap` |
| `shake` | rewrite old tool bodies in place to `artifact://` stubs | **rejected** (07 §14 in-place prune) |
| `handoff` | LLM writes a handoff document; that document *is* the summary | skip — another prompt, same epoch shape as `summary` |
| `remote` | OpenAI `/responses/compact` or Responses streaming compaction; durable history lives in a provider replay payload | **rejected** — fights append-only `messages` + `epochs` |
| notes-backed windows | model-owned notebook + `new_context` rollover, no summariser | skip — a different product |

omp also prunes tool results in place before compacting, walks
`compaction.methodOrder` on every trigger, and lets `/compact [mode]
[focus instructions]` override for one call without mutating settings.
We take the last idea (one-shot override + a stored default) and none of
the rest.

## 2. Styles

Each style is a function of the same inputs 07 §8.4 already has:

```
keep_from, M[0..n), previous head, roster, todos, runtime, knowledge_read
→ new head: ChatMessage (user role)
```

Then the epoch insert is identical: `store.compact`, `history.drain(..keep_from)`,
`used = 0`, `Compacted` event.

### 2.1 `summary` (default)

Unchanged 07 §8.3–8.4. Cache-hot continuation, `COMPACT_INSTRUCTION`,
retry once with `Text only. ` if the reply is empty or has tool calls.
Head:

```
<context_summary epoch="{n}" style="summary">
{model text}

{roster?}
{todos?}
{runtime?}
{knowledge?}
</context_summary>
```

`COMPACT_INSTRUCTION` stays a const. No per-call focus text in v0
(omp's `/compact keep the auth work`). Extra instruction bytes are
cheap (the prefix is cached) but they are a second prompt to keep in
sync with the headings; add it when a transcript shows the stock
headings losing a thread the operator could have named.

### 2.2 `cut`

No model call. Head is harness-only:

```
<context_summary epoch="{n}" style="cut">
folded {k} messages; no summary. Recent turns follow verbatim.

{roster?}
{todos?}
{runtime?}
{knowledge?}
</context_summary>
```

`k = keep_from` (the count of history rows leaving the window), plus one
if a previous head was folded in. The model is told a cut happened and
still gets the live harness truth it would otherwise have to recall.
The verbatim tail is the working memory; everything before it is gone
from the frame (still in `messages`, still in the TUI above the divider).

This is the "just remove it" style. It is the overflow backstop
(§3.2) because it cannot itself overflow: the new frame is the old
frame minus a prefix, plus a few hundred bytes of wrapper.

Cost: zero tokens, zero latency. Loss: the folded span is not
recoverable except by reading the sqlite `messages` table or scrolling
the TUI. That is the point.

### 2.3 `snap`

No model call. The folded span (`previous head` + `M[0..keep_from)`) is
serialised to text, rasterized to one or more PNG frames, and placed in
the new head as `ContentPart::Binary` image parts after a short text
lead-in:

```
text:  <context_summary epoch="{n}" style="snap">
       Archive of {k} folded messages as images. Read them if you need
       earlier facts. Recent turns follow verbatim.
       {roster?} {todos?} {runtime?} {knowledge?}
image: frame 0 … frame N
text:  </context_summary>
```

genai's `MessageContent` is already a list of `ContentPart` (text /
binary / tool). The epochs row stores `head` as `ChatMessage` JSON, so
images persist and resume without a new column.

**Requires a vision-capable current model.** Catalog `Model` grows an
`image: bool` (models.dev already has the modality; 02's trim currently
drops it). Manual `/compact snap` on a text-only model is
`Err("snap needs a vision model (this one is text-only)")`. Auto
falls back to `cut` (§3.2).

**Rasterizer** (`agent::snap`, not this doc's first implementation
slice): bundled public-domain bitmap font, `image` crate, deterministic
layout. Shape follows the session model's family when known (Claude /
Gemini / GPT bill images differently); unknown → one conservative
black-on-white 8×13 frame width. Tool results in the archive are
head+tail truncated (same 32k spirit as `spill`, a smaller cap — 2k —
because pixels are the budget now). No network, no API key.

**Not** oh-my-pi's foveation / 80-frame default / shape-eval table in
v0. One font, one wrap width, hug the printed rows. Revisit if a
transcript shows unreadable frames or billed-token blowups.

If the folded span rasterizes to zero frames (empty), `snap` degrades to
`cut` rather than emitting a head with no images and a lying caption.

## 3. Default algorithm

### 3.1 Resolve

```
requested = override ?? cfg.compact_style     // CompactStyle::Summary if unset
reason    = Auto | Manual | Overflow

if requested == Snap && !model.image:
    if reason == Manual: Err("snap needs a vision model…")
    else:                requested = Cut

if requested == Snap && !snap_built():
    if reason == Manual: Err("snap not built")
    else:                requested = Cut

run(requested)
```

`override` is `Some` only for `/compact <style>` (and the matching RPC
argument). Auto-compaction (07 §8.1) and overflow (07 §8.5) pass
`None`. `/compact` with no args passes `None`.

### 3.2 Overflow

07 §8.5 already: on a provider overflow, compact once with the threshold
ignored, then retry the round. The summariser call itself overflowing
retries once with the tail stripped.

Add: if the resolved style is `summary` and that summariser path fails
(overflow on the second try, empty/tool-call after the `Text only. `
retry), **run `cut` on the same `keep_from`** and continue. History is
not left full. The `Compacted` event's `style` is `cut` so the UI does
not claim a summary exists.

If `cut` still cannot help (`keep_from == 0`), surface the original
overflow and stop the turn — same as today.

Snap during overflow is allowed (no LLM, cannot overflow the summariser).
If the model is not vision, §3.1 already rewrote it to `cut`.

### 3.3 Auto

Threshold fire (07 §8.1) uses `cfg.compact_style` through §3.1. No
second style is tried on success. A `summary` that fails falls into
§3.2's cut only when the failure *is* an overflow/empty summariser;
other errors (`turn in flight` does not apply mid-turn; network) emit
`AgentEvent::Error` and leave history untouched, as today.

Pressure note (07 §8.7) is unchanged: it fires near the threshold
regardless of style. For `cut`/`snap` it is slightly less necessary
(nothing will ask the model to write headings) but still useful — the
tail is what survives.

## 4. Slash, RPC, Signal

### 4.1 `/compact`

```
/compact                      run default style on the shown session
/compact summary|cut|snap     run that style once (config untouched)
/compact style                list; mark the default
/compact style summary|cut|snap
                              persist default to config.yaml
```

Unknown first token: `Err` / `Entry::Info("usage: /compact [summary|cut|snap|style …]")`.
Do not treat leftover words as focus instructions (omp does; we have
none). `/compact foo` is a usage error, not a summary of "foo".

Listing (`/compact style`):

```
compact style: summary (default)
  summary  cache-hot LLM fold (07 §8.3)
  cut      drop the folded span, keep the tail, no model call
  snap     rasterize the folded span to images (vision model)
```

Persisting rewrites the `agent.compact_style` line in place, same
mechanism as `/model` rewriting `agent.model`. Missing key: insert it
next to `compact_at`. Session actors already hold `AgentConfig`; a
config rewrite is picked up the way other live config is (plugin reload
/ next `section::<AgentConfig>`). Until that is wired, the supervisor
updates the in-memory `AgentConfig` it cloned at start so the next
auto-compact in this process sees the new default without a restart.

Bare `/compact` still refuses while a turn is in flight (`Err("turn in
flight")`). `/compact style` is metadata and always runs.

TUI `COMMANDS` row: `("/compact", "[summary|cut|snap|style …]", …)`.
Tab-complete the four tokens (`summary`, `cut`, `snap`, `style`) the
way `/provider` completes `add`/`rm` if that helper exists; otherwise
the slash popup's trailing-space-on-args is enough.

### 4.2 Messages

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactStyle { Summary, Cut, Snap }

impl Default for CompactStyle { fn default() -> Self { Self::Summary } }

pub struct Compact {
    pub session: SessionId,
    /// `None` = configured default, then §3.1.
    pub style: Option<CompactStyle>,
}

pub struct SetCompactStyle { pub style: CompactStyle }  // ask → ()
pub struct CompactStyleInfo { pub current: CompactStyle } // or just return CompactStyle
```

`Compact` is no longer `Copy` if we later add focus text; with only
`Option<CompactStyle>` it stays `Copy`. Keep it `Copy`.

`CompactNow` on the session actor gains the same `Option<CompactStyle>`.

`AgentEvent::Compacted` gains `style: CompactStyle`. `HistoryEntry::Compacted`
gains `style: CompactStyle` (default `Summary` on old rows).

RPC:

```
compact(session, style?: "summary"|"cut"|"snap") → epoch
compact_style() → "summary"|"cut"|"snap"
set_compact_style(style) → ()
```

Signal `/compact` parses the same args; `/compact style` replies with
the listing text. No QR, no extra verbs.

### 4.3 UI

Divider: `── context compacted (epoch {n} · {style}) ──`. Epoch-only
when `style == summary`, so existing screenshots stay honest if we
prefer; **always show the style** — three extra characters, and `cut`
must not look like a summary the operator can Ctrl+O. Ctrl+O on a `cut`
expands the harness stub (folded N, roster, todos). Ctrl+O on `snap`
expands the lead-in text; the images are in the model frame, not the
TUI (the TUI does not render PNG in v0). The scrollback above the
divider is still the full transcript (07 §8.4).

`compacting…` stays. `cut` and `snap` are fast enough that the flag may
not paint; that is fine.

## 5. Config

```yaml
agent:
  compact_at: 0.8          # unchanged (07 §8.1); 0 disables auto
  compact_style: summary   # summary | cut | snap
```

Two keys, still. Not nested (`compact: { at, style }`): that would
break every existing `compact_at` line for a rename. Unknown
`compact_style` value: warn at load, treat as `summary`.

No `compact_model`, no `reserveTokens`, no `keepRecentTokens`, no
`methodOrder`, no `midTurnEnabled` (we already compact mid-turn at the
same threshold), no `idleEnabled`, no `handoffSaveToDisk`.

README config block and the `/compact` sentence in the TUI paragraph
gain the style args.

## 6. Types (as built, when implemented)

```rust
// agent::compact
pub enum CompactStyle { Summary, Cut, Snap }
pub const COMPACT_INSTRUCTION: &str;                 // unchanged
pub fn keep_from(history: &[ChatMessage], limit: u32) -> usize;
pub fn is_overflow(err: &str) -> bool;
pub fn est(m: &ChatMessage) -> u32;

// agent::snap  (later slice)
pub fn rasterize(folded: &[ChatMessage], model: &Resolved) -> Vec<Vec<u8>>; // PNGs

// agent (lib.rs)
pub struct Compact { pub session: SessionId, pub style: Option<CompactStyle> }
pub struct SetCompactStyle { pub style: CompactStyle }
AgentEvent::Compacted { …, style: CompactStyle }
HistoryEntry::Compacted { epoch, summary, style: CompactStyle }

// AgentConfig
compact_at: f32           // 0.8
compact_style: CompactStyle  // Summary
```

`summary` in `HistoryEntry::Compacted` remains the expandable text (model
prose, or the cut stub, or the snap lead-in). It is not renamed.

## 7. Verification

1. **Cut rules unchanged.** Existing `compact::tests::cut_rules` still
   pass. Styles do not touch `keep_from`.
2. **Resolve.** Table: `(override, cfg, image, snap_built, reason) →
   style or Err`. Covers: bare default, one-shot cut, auto snap on
   text-only → cut, manual snap on text-only → Err, auto snap before
   rasterizer → cut.
3. **Head tag.** `summary` / `cut` / `snap` each produce
   `style="…"` on the wrapper; harness lines still present when state
   is non-empty. A head with no attribute round-trips as `Summary`.
4. **Slash.** `/compact` sends `Compact { style: None }`; `/compact cut`
   sends `Some(Cut)`; `/compact style` does not compact; `/compact style
   snap` persists; `/compact foo` is usage, not a focus string.
5. **Overflow → cut.** Summariser overflow after the existing retry
   opens an epoch with `style=cut` and the turn retries; `keep_from ==
   0` still surfaces the original error.
6. **Prefix oracle (07 §12.1).** A scripted session that cuts (no
   summariser) still has byte-prefix frames *within* the new epoch.

No live-model e2e for `cut` (no tokens). `summary` e2e is already 07
§12.5. `snap` e2e waits on the rasterizer: a fixture PNG list in the
head, resume, `ContentPart` count.

## 8. Rejected

- **omp `methodOrder` as config.** Five fallbacks, per-reason skips
  (handoff forbidden on overflow), speculation of the first LLM method.
  We have three styles and two fallbacks. A list is a knob the operator
  will set wrong.
- **`shake` / in-place tool-result prune.** 07 §14. Bound at append
  (`spill`); the fold is the only time old bodies leave the window.
- **Provider-native remote compact.** Replacement history is not our
  `messages` table. Resume would need a second source of truth.
- **Handoff as a style.** Same epoch as `summary`, different prompt.
  Add a fourth style when someone wants the document more than the
  headings; not before.
- **Focus instructions on `/compact`.** Conflicts with style tokens
  (`/compact cut` must never mean "summarise with focus 'cut'"). Revisit
  as `/compact summary -- focus…` if needed.
- **Per-session style.** Global config + one-shot override. A session
  field is a third place to look.
- **Utility / `compact_model`.** 07 §14: the session model is the cheap
  one because the prefix is cached. `cut`/`snap` do not call a model.
- **Nested `agent.compact.{at,style}`.** Breaks `compact_at`.
- **Rendering snap PNGs in the TUI.** The model needs them; the operator
  still has the scrollback. iocraft image views are out of scope.
- **Importing `@oh-my-pi/snapcompact` or the napi native.** Wrong
  language, wrong shape table, wrong dependency. If we rasterize, we
  rasterize in `agent::snap`.
- **Silent `snap` → `summary`.** Manual snap that cannot run must not
  spend tokens the operator tried to avoid. Auto may `cut`.

## 9. Edits to other docs

Applied 2026-09-10. `Model.image` waits on snap.

- **07 §8.3–8.6.** Summariser is the `summary` style, not the only
  style. `Compact.style: Option<CompactStyle>`. Overflow may finish as
  `cut`. Head wrapper gains `style=`.
- **07 §11 / §13.** `CompactStyle`, `compact_style` config, `/compact`
  args. Implementation order gains a styles step after the existing
  compaction step.
- **07 §14.** Does not reject `cut`/`snap`; they are not in-place
  rewrites. The rejected row for "a separate utility model" stays.
- **02 §4.** `/compact [summary|cut|snap|style …]`.
- **README.** `compact_style` next to `compact_at`; TUI sentence names
  the args.
- **02 catalog (when snap is built).** `Model.image: bool` from
  models.dev, default false when absent.

## 10. Implementation order

Each step leaves `wth` runnable. Snap is last on purpose.

1. **`CompactStyle` + plumbing, behaviour still `summary`.** Enum,
   `AgentConfig.compact_style`, `Compact.style: Option<_>`, thread
   through supervisor / `CompactNow` / `Turn::compact`, persist `style`
   on events and `HistoryEntry` (readers default missing to `summary`).
   `/compact` with no args still summarises. Tests: serde + default.
2. **`cut`.** Head builder without a model call. `/compact cut`. Overflow
   summariser-failure → cut. Resolve table tests. TUI/Signal/RPC args.
3. **`/compact style` listing + persist.** `SetCompactStyle` rewrites
   `config.yaml` and the live `AgentConfig`. Listing text.
4. **Divider + docs.** Style on the TUI/print divider; 07/02/README
   edits in §9; this doc → as-built.
5. **`snap` (later).** `Model.image`, `agent::snap` rasterizer, image
   parts on the head, `/compact snap`, auto fallback to cut when
   `!image`. Fixture test with a canned PNG. Planned in `13-snap.md`.

v0 for the operator is steps 1–4: they can choose `summary` or `cut`,
set the default, and auto/overflow do the right thing. Snap is a named
hole with a specified head, not a surprise later.
