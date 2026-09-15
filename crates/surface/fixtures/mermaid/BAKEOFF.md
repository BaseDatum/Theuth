# Mermaid → text bake-off (design 30 §2.2, §13 step 1)

Harness: throwaway cargo bin at `/tmp/mermaid-bakeoff` (one binary, three cargo
features, one `render(&str)` per crate). Each fixture rendered on its own thread
behind a 5 s `recv_timeout`, `catch_unwind` around the call; every fixture run
twice, warm (min) time reported. Toolchain: stable `cargo 1.96.1`, release
profile, Apple M3 Max. All three crates compiled on stable.

Crates: `merman 0.7.0` (`features = ["ascii"]`), `mmdflux 2.6.1`
(`default-features = false`; lib API `render_diagram`), `mermaid-text 0.57.0`.

## Results (12 fixtures × 3 crates)

Status: `ok` = rendered; `unsup` = crate reports the diagram type/feature as
unsupported (`Ok(None)` or an explicit unsupported variant); `err` = parse
error. Width = widest line in chars. Time = warm wall, microseconds. No crate
panicked or hung on any fixture.

| Fixture | merman 0.7 | mmdflux 2.6 | mermaid-text 0.57 |
|---|---|---|---|
| 01-flowchart-lr (8 nodes, labelled edges, quoted `()` labels) | ok · w85 · 369 µs | ok · w212 · 491 µs | ok · w107 · 1156 µs |
| 02-flowchart-td-subgraphs | ok · w26 · 290 µs | ok · w23 · 860 µs | ok · w25 · 448 µs |
| 03-sequence-alt (4 participants, alt/else, loop) | ok · w49 · 271 µs | ok · w69 · 88 µs | ok · w80 · 83 µs |
| 04-state (stateDiagram-v2, 6 states) | unsup · `AsciiError::UnsupportedDiagram { state }` | ok · w56 · 468 µs | ok · w133 · 318 µs |
| 05-class (`"1" *-- "*"` endpoint labels) | unsup · `AsciiError::UnsupportedFeature { class, "relationship endpoint labels" }` | ok · w55 · 214 µs | err · `parse error: classDiagram: unrecognised line: "Spec \"1\" *-- \"*\" Element : contains"` |
| 06-er | err · `Diagram parse error (er): UnrecognizedToken { token: (59, Identifying, 61), expected: ["Name"] }` (crate bug: rejects valid `\|\|--o\|`; `\|\|--o{` and `}o--\|\|` render) | unsup · `detect_diagram → None` | ok · w62 · 64 µs |
| 07-pie | unsup · `AsciiError::UnsupportedDiagram { pie }` | unsup · `detect_diagram → None` | ok · w80 · 31 µs (bar chart) |
| 08-gantt | unsup · `AsciiError::UnsupportedDiagram { gantt }` | unsup · `detect_diagram → None` | ok · w57 · 37 µs (bar chart) |
| 09-flowchart-wide (14 nodes, one LR row) | ok · w313 · 270 µs | ok · w328 · 379 µs | ok · w313 · 210 µs |
| 10-flowchart-1000 (1000-node chain) | ok · w12 · 22.5 ms · 9995 lines | ok · w12 · 63.1 ms · 5996 lines | ok · w14 · 439 ms · 7995 lines |
| 11-syntax-error (`A[fetch(url)]`, unquoted parens) | err · `Diagram parse error (flowchart-v2): User { error: LexError { message: "Invalid text label: contains structural characters; quote it to use them" } }` | ok · w86 · 236 µs (lenient: accepts what mermaid.js rejects) | ok · w69 · 122 µs (lenient) |
| 12-flowchart-cycle (A→B→C→A, diamond) | ok · w17 · 314 µs | ok · w14 · 229 µs | ok · w23 · 133 µs |
| **rendered / unsupported / error / panic** | **6 / 5 / 1 / 0** | **8 / 3 / 0 / 0** | **10 / 0 / 1 / 0** |

Extra probes (not fixtures): merman renders `xychart-beta`; `mindmap`/`gitGraph`
→ `AsciiError::UnsupportedDiagram`; garbage text → `merman::Error::DetectType`
(`No diagram type detected…`); `B --> end` → parse error (mermaid.js also
rejects `end` as an id). mmdflux `detect_diagram` → `None` for garbage,
mindmap, gitGraph, ER; a truncated edge gives `Parse error at line 2, column
9: expected identifier or edge_label` (line/col located). merman parse errors
carry byte offsets (`token: (21, Sep, 22)`), not line/col.

### Dependency weight and compile time

| Crate | `cargo tree` crates (fresh project) | `cargo tree -d` duplicate versions | crates **new to the wth workspace** (`Cargo.lock` name diff) | cold `cargo build --release` of the harness |
|---|---|---|---|---|
| merman 0.7 (`ascii`) | 139 | 71 | 8: `merman merman-ascii merman-core htmlize json5 lalrpop-util lol_html ryu-js` | 33.9 s |
| mmdflux 2.6 (no `cli`) | 30 | 12 | 1: `mmdflux` | 23.2 s |
| mermaid-text 0.57 | 8 | 0 | 2: `mermaid-text ascii-dag` | 10.2 s |

merman's fresh-tree count is inflated by `url`/`idna`/`icu_*`/`cssparser`
(pulled by `lol_html` + `htmlize` for HTML label handling); the workspace
already carries that stack, so the real delta is 8 crates.

### Output quality notes (from `/tmp/mermaid-bakeoff/out/<crate>/*.txt`)

- **merman** flowcharts: tidy orthogonal routing, edge labels inline on the
  edge, nodes padded to a 3-row box (tall, but ≤ 100 cols for 01/02/12). One
  label collision on 01 (`append(entry)`/`replay`/`retry` overprint as
  `appreplaytry)`). Sequence: narrowest, clean `alt`/`else`/`loop` frames.
- **mmdflux** flowcharts: compact single-row boxes, best subgraph frames
  (`┌── Client ──┐`), best state and class output; but labelled-edge LR
  layout on 01 degenerates (212 cols, labels floating far from edges,
  `Desktop` placed above `TUI`). Sequence good. Ignores the syntax error in 11.
- **mermaid-text**: widest coverage (pie/gantt render as bar charts), has a
  `render_with_width(src, Some(cols))` budget the others lack; but leaks the
  quote characters into node labels (`│ "TUI (ghostty)" │`), state diagram
  is 133 cols wide, sequence frames are filled with `░` shading, 1000-node
  chain takes 439 ms, and it silently accepts the syntax error in 11.

## Ranking (criteria in §2.2 order)

1. **No panic on all 12** — tie: all three, none hangs.
2. **Sequence support** — all three render 03; merman is the cleanest and
   narrowest (49 cols vs 69 vs 80). merman ▸ mmdflux ▸ mermaid-text.
3. **Width at 100 cols** — on the fixtures all three render (01/02/03/09/12),
   merman is ≤ 100 on four, mmdflux and mermaid-text on three (01 is 212 and
   107). Everyone overflows on 09 (by design; that is the `None` → source
   fallback). merman ▸ mermaid-text ▸ mmdflux.
4. **Dependency weight** — mermaid-text ▸ mmdflux ▸ merman (8 new crates,
   34 s cold build).

**Winner: `merman`** — wins criteria 2 and 3 outright, and is the only parser
that agrees with mermaid.js: it rejects the unquoted-parens fixture with a
located, actionable message, so the `<surface_error>` round (§5) fires for the
same blocks the desktop `mermaid` plugin would fail on. The two lenient crates
would render 11 in the TUI while the desktop shows an error card, and the
model would never be corrected. Its error enum also separates "unsupported"
from "syntax error" without string matching. It loses only on weight, and the
delta against the existing workspace is 8 crates.

Costs to accept, and what to do about them:

- `stateDiagram-v2` is not rendered by `merman-ascii` 0.7 → falls back to
  the source block. Either keep `stateDiagram-v2` in the §6.2 prompt list
  (falls back gracefully; no correction round fires) or drop it until merman
  0.8 ships state ASCII. mmdflux is the only candidate that draws state today.
- `classDiagram` with relationship endpoint labels (`"1" *-- "*"`) →
  `UnsupportedFeature` → source fallback. Plain relationships render.
- `erDiagram` zero-or-one cardinality `o|` is rejected by merman's lexer
  (valid mermaid). Report upstream; until then such ER blocks would trigger a
  correction round the model cannot satisfy — treat `Diagram parse error (er)`
  as unsupported (fallback, no correction) if this shows up in the log.
- Parse errors carry byte offsets, not line/col; `surface::mermaid_lines`
  can map offset → line for the `<surface_error>` text.

Runner-up `mmdflux` if state/class matter more than syntax parity and
labelled LR flowcharts turn out rare. Per §2.2 the loser is not kept behind a
feature flag.

## Winner: dependency line and call

```toml
merman = { version = "0.7", features = ["ascii"] }
```

```rust
use merman::ascii::{AsciiError, HeadlessAsciiError, HeadlessAsciiRenderer};

/// `Ok(Some)` rendered · `Ok(None)` unsupported (fall back to source) · `Err(msg)` syntax error.
pub fn render(src: &str) -> Result<Option<String>, String> {
    match HeadlessAsciiRenderer::new().render_ascii_sync(src) {
        Ok(text) => Ok(text),
        Err(HeadlessAsciiError::Parse(merman::Error::UnsupportedDiagram { .. }))
        | Err(HeadlessAsciiError::Ascii(AsciiError::UnsupportedDiagram { .. } | AsciiError::UnsupportedFeature { .. })) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
```

Error type map (`merman::ascii::HeadlessAsciiError`):

| Variant | Meaning | Action |
|---|---|---|
| `Ok(None)` | engine produced no render model | fallback |
| `Parse(merman::Error::UnsupportedDiagram { diagram_type })` | type known to mermaid, not to merman-core | fallback |
| `Ascii(AsciiError::UnsupportedDiagram { diagram_type })` | parsed, but no text renderer (state, pie, gantt, mindmap, gitGraph…) | fallback |
| `Ascii(AsciiError::UnsupportedFeature { diagram_type, feature })` | parsed, one construct not drawable (class endpoint labels) | fallback |
| `Ascii(AsciiError::RenderLimitExceeded { actual, limit })` | > `max_grid_cells` (250 000 default) | fallback (resource guard; the 1000-node chain stays under it) |
| `Parse(merman::Error::DiagramParse { diagram_type, message })` | syntax error | `<surface_error>` |
| `Parse(merman::Error::DetectType(_))` | not a mermaid diagram at all | `<surface_error>` |
| `Parse(merman::Error::{MalformedFrontMatter, InvalidDirectiveJson, InvalidFrontMatterYaml})` | bad `---` front-matter / `%%{ }%%` directive | `<surface_error>` |

`HeadlessAsciiRenderer::new()` costs ≈ 0.2 ms (included in the timings
above); construct per call or once in a `OnceLock` — it is `Clone` and holds
only registries + config. `.with_charset(AsciiCharset::Ascii)` gives a
`+-|` rendering for non-Unicode terminals. `AsciiRenderOptions` has no
column budget; width policy is the caller's (`lines().map(width).max() >
cols` → fallback).
