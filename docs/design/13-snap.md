# 13 — Snap compaction: build plan and the decisions 11 left open

Status: as-built v0 · 2026-09-11 · completes `11-compaction-styles.md` §2.3 / §10
step 5. Where this doc disagrees with 11, this doc wins; §9 lists the edits.
Verified live (claude-sonnet-4-5, headless): 11 turns of `read_file` grew a
session to 40.9k tokens; `compact(session, "snap")` folded 19 messages into
4 frames (1568×1568 ×3 + 1568×1520, 33–48 KiB each, 226 KB epoch row);
asked without tools for every `offset` value in the folded span the model
listed all ten correctly from the images. Frame text is crisp at 11×16.
Measured bill: the next single call was 34.9k input tokens against ~8.5k
system + ~8.2k kept tail, so the four frames cost ~18k — **~4.5k per full
frame**, 1.4× omp's 3,136 figure; `snap::shape("anthropic")` now carries
the measured number. Auto-compaction was not reached in this run
(`compact_at: 0.15` on claude-sonnet-4-5's 1M-token window is 150k; the
session stopped at 41k) — the trigger path is unchanged from 07. Not
exercised: OpenAI/Gemini frames, manual snap on a text-only model.

11 named `snap` as "a style in v0's contract, a later crate in the build" and
specified the head shape. Nothing was built: `compact::snap_built()` returns
`false` (`compact.rs:129-131`), `resolve()` rewrites auto/overflow snap to
`cut` and manual snap to `Err("snap not built")` (`compact.rs:134-149`), and
the `CompactStyle::Snap` arm in `Turn::compact` (`loop_.rs:1445`) is dead
code behind it. Every UI advertises the token (`/compact snap`, listing text,
RPC doc comment) and forwards it. `models::Model` has no `image` flag. No
rasterizer, no font, no PNG dependency anywhere in the tree.

This doc does three things 11 did not: (1) states the token economics so
the feature is built for what it actually is, (2) fixes the geometry, font,
serialisation, budget and gates so the implementation has no open choices,
(3) lists the code sites.

## 0. Decisions in one screen

1. **Snap is a fixed-budget lossless archive, not compression.** On
   Anthropic/OpenAI a full frame bills ~3k tokens and holds ~14k chars
   (~3.5k text tokens): 1–1.2×. Only Gemini (fixed 1,120 tokens per image)
   compresses (~5×). So the archive gets the **same budget as the kept
   tail** (`limit / 8` tokens, 07 §8.2), spent on the *most recent* part of
   the folded span. Earlier messages are dropped exactly as `cut` drops
   them. Caption says so.
2. **One geometry per provider family, three rows, taken from oh-my-pi's
   legibility bench** (`~/Develop/oh-my-pi/packages/snapcompact/src/snapcompact.ts`
   header): 8×13 glyphs; Anthropic 11 px advance × 16 px pitch on a 1568 px
   frame; OpenAI 8 × 22 on 1568; Google 8 × 22 on 2048; unknown → the
   Anthropic row. Frame height hugs the rows actually printed. No foveation,
   no shape eval, no per-model table.
3. **Font: X.org misc `8x13.bdf`, public domain, embedded with
   `include_str!`, parsed by ~40 lines.** ISO-8859-1 coverage; anything
   else prints `?`. No font crate.
4. **PNG via the `png` crate, 8-bit grayscale.** One new dependency
   (`flate2`/`miniz_oxide` are already in the lock). Not `image`.
5. **Gate = `Model.image && transport != Cursor`.** `Model.image: bool`
   comes from models.dev `modalities.input ∋ "image"` (the trim recipe
   drops modalities today). The Cursor backend serialises text parts only
   (`crates/cursor/src/history.rs:236-252`), so it is text-only for this
   purpose regardless of the model. Bare passthrough ids (not in the
   catalog) are `image: false`.
6. **No token-estimate change.** `compact::est` only prices `history`
   rows; the head is priced by the provider's reported usage on the first
   call after the cut (`used`). A head full of base64 never goes through
   `est`. The frame budget in (1) uses the family's per-frame estimate,
   nothing else. Superseded in part by 36 §6.5: `used` is now seeded with
   `est(head) + Σ est(tail)` after the fold instead of 0, so a `snap` head
   is priced once by `est` (text + `IMAGE_TOKENS` per frame) until the
   provider reports.
7. **`snap_built()` goes away.** With the rasterizer in-tree the only gate
   is (5). `resolve()` gains the model/transport argument and the
   `Err("snap needs a vision model (this one is text-only)")` branch 11
   §3.1 specified.

## 1. Economics (the part 11 skipped)

Measured per-frame bills (omp's live benchmarks, quoted in the snapcompact
header; the Anthropic row replaced by our own measurement, Status above):

| family | frame | bill / frame | cells (cols × rows) | chars / frame | text-token equiv (÷4) | ratio |
|---|---|---|---|---|---|---|
| anthropic | 1568², 11×16 | **~4,500 measured** (omp: 3,136) | 142 × 98 | ~13,900 | ~3,500 | 0.8× — costs more than the text it replaces |
| openai | 1568², 8×22 | ~2,881 (32 px patches ×1.2) | 196 × 71 | ~13,900 | ~3,500 | 1.2× |
| google | 2048², 8×22 | 1,120 (fixed `media_resolution`) | 256 × 93 | ~23,800 | ~6,000 | 5.3× |

Consequences:

- A 160k-token session compacting at 0.8 has `limit / 8 = 16k` tokens of
  archive budget: **5 frames on Anthropic (~70k chars of the fold), 14 on
  Gemini (~330k chars)**. The rest of the fold is gone, as with `cut`.
- Steady state: every later request carries the archive in the cached
  prefix. `summary` heads are ~1–2k tokens; a snap head is up to 16k. At
  Anthropic cache-read pricing that is ~1.6k tokens-equivalent per call —
  cheaper than one summariser call after ~2 rounds, more expensive than
  `cut` forever.
- What snap buys is therefore: **no summariser call, no summariser
  drift/hallucination, verbatim recent history at a fixed price, and an
  archive the model can actually read.** It does not buy a smaller frame.
  If a transcript later shows the model never reading the frames, `cut`
  is strictly better and snap should be removed, not tuned.

## 2. Rasterizer (`agent::snap`)

### 2.1 Serialisation of the folded span

Input: `previous head` (if any) followed by `full[..keep_from]`. Output: one
`String`, one logical line per part, hard-wrapped to the family's column
count with a two-space continuation indent. Tabs → 4 spaces; `\r` dropped;
chars outside 8×13's charset → `?`.

```
== epoch 2 head ==            ← previous <context_summary>, text parts only
[user] …
[assistant] …
[tool_call read_file {"path":"…"}]     ← args as compact JSON, capped 200 chars
[tool read_file] …                     ← result body, head 1,400 + tail 600 chars
                                         with "[… n chars omitted …]" between (11 §2.3: 2k cap)
```

Reasoning/thought-signature parts are skipped (never re-sent anyway).
Binary parts inside the fold (an earlier snap head) print as
`[image frame omitted]`; snap does not re-rasterize pixels.

### 2.2 Budget → which lines

`budget_tokens = limit / 8` (same `limit` `Turn::compact` computes:
`compact_at × window`, or `window`). `frames_max = max(1, budget_tokens /
family.tokens_per_frame)`. `rows_max = frames_max × family.rows`.

Take the **last** `rows_max` wrapped lines of the serialisation
(`snap::budget_lines(lines, budget_tokens, &shape) -> (lines, dropped)`). If
lines were dropped, the first printed line is `[… {dropped} earlier lines
dropped …]` and the caption (§2.4) repeats the count. If nothing is printed
(empty fold — cannot happen with `keep_from > 0`, but the branch exists),
`rasterize` returns no frames and `Turn::compact` degrades to `cut` (11
§2.3): `style = Cut`, body = the cut body.

### 2.3 Drawing

`Shape { cell_w, cell_h, frame: u32, tokens_per_frame: u32 }` per family
(§0.2). Glyph bitmaps from the embedded BDF: 8 columns × 13 rows, drawn at
the cell's top-left; the extra advance/leading is white. Black (`0`) ink on
white (`255`), no anti-aliasing, no dimming (omp's bench: black beat dim).
Frame width = `frame`; height = `rows_in_this_frame × cell_h`, so a partial
last frame bills only what it prints. Encode 8-bit grayscale PNG, default
compression. Deterministic: same input → same bytes.

```rust
// agent::snap
pub struct Shape { pub cell_w: u32, pub cell_h: u32, pub frame: u32, pub tokens_per_frame: u32 }
pub fn shape(adapter: &str) -> Shape;                      // "anthropic" | "openai" | "gemini" | _
pub fn serialise(head: Option<&ChatMessage>, folded: &[ChatMessage], cols: usize) -> Vec<String>;
pub fn rasterize(lines: &[String], shape: &Shape) -> Vec<Vec<u8>>; // PNGs, ≥1 unless lines empty
```

~250 lines including the BDF reader. Pure functions, no I/O, testable
without a model.

### 2.4 Head

```rust
// Turn::compact, after the harness parts (roster / todos / runtime / knowledge) are built
let mut parts = vec![ContentPart::Text(compact::wrap_head_open(n, Snap, caption, harness))];
parts.extend(pngs.into_iter().enumerate().map(|(i, png)| ContentPart::from_binary_base64(
    "image/png", BASE64.encode(png), Some(format!("frame-{i}.png")))));
parts.push(ContentPart::Text(compact::HEAD_CLOSE.into()));
ChatMessage::user(MessageContent::from_parts(parts))
```

Caption, with `k = keep_from + (previous head ? 1 : 0)` (the same count the
cut body reports) and `f = pngs.len()`:

```
Archive of {k} folded messages as {f} image(s), most recent last{; the earliest {dropped} lines were dropped}. Read them if you need earlier facts. Recent turns follow verbatim.
```

The `{; …}` clause appears only when `budget_lines` dropped lines. Harness
lines follow the caption inside the first text part, exactly where
`wrap_head` puts them today: `wrap_head` = `wrap_head_open` + `"\n"` +
`compact::HEAD_CLOSE`, so `summary`/`cut` keep producing one string.

Readers already cope: `style_of_head` parses the first text part
(`compact.rs:170-187`); `text_of` joins text parts (`loop_.rs:1681`) so the
TUI divider shows caption + harness lines and Ctrl+O expands them (11
§4.3); `Frame::request` forwards any `MessageContent` (`frame.rs:68-90`);
`Store::compact` stores the `ChatMessage` JSON (base64 inline — 5 frames ×
~60–120 KiB ≈ 0.5 MiB per epoch row, acceptable; note in 07 §2.4).

## 3. Gate and resolve

```rust
pub(crate) fn resolve(
    r#override: Option<CompactStyle>, cfg: CompactStyle, reason: CompactReason, vision: bool,
) -> Result<CompactStyle, String> {
    let requested = r#override.unwrap_or(cfg);
    if requested == Snap && !vision {
        return match reason {
            Manual => Err("snap needs a vision model (this one is text-only)".into()),
            Auto | Overflow => Ok(Cut),
        };
    }
    Ok(requested)
}
```

`vision = matches!(current.backend, Backend::Genai { .. }) &&
current.model.model.as_ref().is_some_and(|m| m.image)`, computed in
`Turn::compact` from `Current` (`Resolved.model: Option<models::Model>`;
bare passthrough ids are `None` → text-only). The backend check stands in
for "transport != Cursor": `Backend::Cursor` is the only text-only
transport and `Current` names the backend directly. The frame shape is
`snap::shape(models::Catalog::adapter(current.model.provider))`; a missing
provider takes the Anthropic row. `models::Model.image: bool`
(`#[serde(default)]`) is set by the trim recipe from `modalities.input`,
and `ModelInfo` (02 §2) carries `image` so `/model` can mark vision models.

Overflow: snap allowed (no LLM). The overflow retry sends the new frame
with images; if the *provider* rejects images (a catalog lie), the error
surfaces like any other model error and the next overflow attempt in that
turn is refused as today (`self.overflowed`). Not special-cased.

## 4. Surfaces

No new commands. Existing `/compact snap`, `/compact style snap`, RPC
`compact(session, "snap")`, Signal `/compact snap` start working. Listing
text drops nothing; the RPC doc comment "Snap is not built yet" goes.
`ModelInfo.image` shows as `· vision` in `/model` rows (TUI + Signal) so
the operator can tell why snap was refused.

## 5. Config

None new. `agent.compact_style: snap` is already accepted. The budget is
derived (`limit / 8`), the shape is derived from the adapter. A
`snap_budget` knob returns only if a transcript shows the default too small
or too costly; until then it would be a knob set wrong.

## 6. Verification

1. `snap::tests::bdf_parses_all_printable_ascii` — 95 glyphs, each 13 rows.
2. `snap::tests::rasterize_is_deterministic_and_hugs_rows` — two calls, same
   bytes; decode with `png::Decoder`, width = `frame`, height =
   `rows × cell_h`, at least one black pixel in the glyph rows of line 0.
3. `snap::tests::budget_keeps_the_tail` — 300 lines, `frames_max = 1`
   (98 rows) → the drop marker is line 0 and the last input line is the
   last output line.
4. `compact::tests::resolve_table` — the `vision` column: manual +
   text-only → Err; auto/overflow + text-only → Cut; vision → Snap.
   `compact::tests::wrap_head_open_plus_close_equals_wrap_head` pins the
   split.
5. `store::tests::snap_head_round_trips_parts`: a snap head with two
   rasterized frames, `compact`, `frame` → `head.content.parts().len() ==
   4`, `style_of_head == Snap`, `text_of` contains the caption and the
   closing tag, JSON identical.
6. Live (07 §12.5 shape): a real Anthropic session, `/compact snap`, then
   ask for a token that only exists in the archived span (a filename read
   in turn 1). Record in this doc's Status: frames, bytes, the provider's
   reported input tokens for the next call versus §1's estimate.

## 7. Rejected

- **Full-fold archive (unbounded frames).** §1: on Anthropic it costs as
  much as the text it replaces, forever. Budget = tail budget.
- **`image` crate.** Pulls decoders for a dozen formats for one grayscale
  encode. `png` alone.
- **A TrueType font via `ab_glyph`/`fontdue`.** Anti-aliased glyphs at
  8–11 px are what omp found falling below the OCR floor; bitmap fonts are
  the point. Also a heavier dependency.
- **Importing omp's shape table / napi renderer.** 11 §8. We take three
  measured rows and the conclusion "black, wide tracking, 8×13", not the
  code.
- **Rendering frames in the TUI.** 11 §8. Scrollback is the archive for
  the operator.
- **Re-rasterizing a previous snap head.** Pixels-of-pixels; print a
  marker.
- **A `Model.image` fallback to `true` when the catalog is silent.**
  Would send images to a model that 400s them on an overflow retry, the
  worst moment. False is the conservative default.

## 8. Implementation order

Each step leaves `wth` green.

1. **`Model.image`.** `models/src/lib.rs` `Model { image: bool }`,
   `bin/trim.rs` keeps `modalities.input`, `ModelInfo.image`, `just models`
   regenerate, `models-check` still clean. TUI/Signal `/model` rows show
   `vision`.
2. **`agent::snap`.** BDF asset (`crates/agent/assets/8x13.bdf`), reader,
   `serialise`, `rasterize`, `shape`; `png` dep; tests 1–3.
3. **Head + resolve.** `wrap_head_open`, multi-part head in
   `Turn::compact`, `resolve(…, vision)`, delete `snap_built`; test 4–5.
   Remove the dead `Snap => Err` arm.
4. **Docs + live check.** 11 §0.7/§10 → point here; 07 §2.4 row note on
   head size; RPC doc comment; README unchanged (key already documented);
   test 6 recorded in Status.

Size: ~300 lines new (`snap.rs` + asset), ~60 changed, one dependency.

## 9. Edits to other docs

- **11 §0.7, §3.1, §6, §10 step 5.** "snap not built" → this doc; `resolve`
  signature gains `vision`; `agent::snap` API is §2.3 here.
- **11 §2.3.** Add the budget sentence (archive = last `limit/8` tokens of
  the fold) and the family table reference.
- **07 §2.4.** `epochs.head` may hold base64 PNG parts; size note.
- **02 §1.** `Model.image` from `modalities.input`; `ModelInfo.image`.
- **00 §12.** Phase row.
