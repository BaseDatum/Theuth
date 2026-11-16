# Design 197 SHAKE-BY-CATEGORY: contract (binding for the engine and desktop workers)

## Sky's ask and rulings (2026-11-16 22:31–22:36)
- "I thought we had a way in the UI to shake specific things from context like images, surfaces, diagrams, etc."
- "but we just get a big list of things to shake. I want to be able to shake and select context by category."
- "don't forget images, i want to be able to shake images from the context, pngs, jpegs, etc. you get me?"
- Rulings:
  - **Diagrams** in replies: cut out just the diagram; the reply text stays with a one-line marker.
  - **Pages, apps and shown files**: swap their source for a marker; the call and its card stay.
  - **Conversation** (user messages, assistant replies, summary): pick items only, with no select-all.
  - Build now.
- Mock (the target look): `mock.html` beside this file.
- Base: design 48 SHAKE (`docs/design/48-shake.md`); everything in it stays true unless changed here.

## 1. Categories (`ContextKind`) and groups
Every inventory item has exactly one `kind` and one `group` (a stable lowercase key).

| kind | what | groups | label / detail |
|---|---|---|---|
| `image` | EVERY image occurrence (MIME `image/*`, any format: PNG, JPEG, GIF, WebP, HEIC, …) anywhere in the active frame | `attachment` (binaries of user or peer rows, including pointing/screen images); `screenshot` (images in results of `computer_screenshot` / `browser_screenshot`); `tool` (every other tool-result image: `read_file`, `read_url`, MCP tools such as Grafana panels, images returned through `python`); `summary` (images of the epoch head) | label: file name, else the tool name. detail: `"PNG · 1568×1103"`: the MIME subtype uppercased (`JPEG` for `image/jpeg`), with dimensions read from the stored bytes' header for PNG/JPEG/GIF/WebP when cheap, else the format only |
| `document` | non-image binaries (native PDF) | `document` | label: file name; detail: format (`"PDF"`) |
| `surface` | (a) surface fences in assistant text: fenced blocks whose info string is exactly `json-render`, `openui` or `mermaid` (target `fence`); (b) the source argument of an assistant call to `show` (field `content`), `app` (`source`) or `page_preview` (`html`) (target `source`) | `diagram` (all fences); `page` (`show` whose `name` ends in `.html`/`.htm`, and `page_preview`); `app`; `file` (any other `show`) | label: json-render → up to three distinct element `type`s in document order (`"MetricGrid, Checklist"`); mermaid → its diagram keyword (`"sequenceDiagram"`); openui → `"openui: <root component>"`; cards → the `name` arg, else `title`, else the tool name. detail: `"in reply"`, `"show"`, `"app"` or `"page_preview"` |
| `tool` | tool-exchange rows, as today | the first `fn_name` of the row (calls or responses) exactly as stored (`python`, `shell`, `read_file`, `grafana_prod__query_prometheus`, …) | label as today (`"tool result: python"`); detail: the other names when there are several |
| `note` | harness-injected rows | `knowledge` (`<knowledge>` hint rows), `todo` (`<todo>` rows), `time` (rows that are only a `<time …/>` stamp), `notice` (every other injected row: child_stalled, job_result, job_idle, reminder, surface_error, cell_result, schedule wrappers, vision notes, system-role rows that are not the head) | label: bounded preview |
| `peer` | messages from other agents (the history entry's `peer` is set) | `peer` | label: sender + preview; detail: the sender address |
| `user` | user-typed rows (role user, not injected, not peer) | `user` | label: bounded preview |
| `assistant` | assistant rows with text (a calls-only row is `tool`) | `assistant` | label: bounded preview |
| `summary` | the epoch head | `summary` | as today |

- `ContextKind::Message` is removed and replaced by `user` / `assistant`; injected rows become `note` and peer rows become `peer`.
- Identify injected rows by the SAME authoritative marker the transcript uses to hide harness rows (find it; do not guess from text when a structural marker exists). Leading-tag matching only chooses the `note` group.
- Snapshot order is unchanged (head, head images, then rows with their binaries in seq order); a row's fences and sources follow its binaries.

## 2. Inventory item (`ContextItem`): additive fields
```rust
pub struct ContextItem {
    pub target: ContextTarget,
    pub kind: ContextKind,
    pub group: String,                 // §1 key
    pub label: String,                 // ≤ 80 chars (existing)
    pub detail: Option<String>,        // ≤ 60 chars
    pub tokens: u32,                   // compact::est of what this target removes
    pub at: Option<String>,            // RFC 3339 of the row, when known
    pub remove_with: Vec<ContextTarget>,
}
```
TS: `detail` and `at` are optional; `group` and `tokens` are required.

**Token rule:**
- A whole-row target counts the row AS CURRENTLY PROJECTED, so it includes its remaining binaries, fences and sources.
- A binary counts `est_part` of that binary.
- A fence counts the estimate of its fence text.
- A source counts the estimate of the argument value it replaces.
- The head counts the projected head.
- The UI sums the closed target set and skips any binary, fence or source whose owner row is also in the set.

## 3. New targets and the mask
```rust
pub enum ContextTarget {
    Message { seq }, Head { epoch }, Binary { owner, part, image },   // unchanged
    /// One surface fence inside one text part of an assistant row.
    Fence { owner: ContextOwner, part: u32, index: u32 },
    /// The source argument of one show / app / page_preview call part.
    Source { owner: ContextOwner, part: u32 },
}
```
Serde stays `tag = "kind"`, snake_case: `{"kind":"fence",…}` and `{"kind":"source",…}`.

**Fence `index`:** the 0-based ordinal of surface fences within that ORIGINAL text part.
- A surface fence opens with a line of three or more backticks or tildes followed immediately by `json-render`, `openui` or `mermaid` and nothing else but whitespace.
- It closes at the next line made of the same fence character, at least as long.
- An unclosed fence runs to the end of the part.
- Other fences (` ```rust `, ` ```bash `, …) are ignored and not counted.
- The desktop uses the same rule (§8), so keep the detector small and documented.

**Source `part`:** the original index of the ToolCall content part. It is offered only when the call's `fn_name` is `show` / `app` / `page_preview` and its field (`content` / `source` / `html`) is a non-empty string. A `show` by `path`, or by `edits`, has no source item.

**Scope and subsumption:**
- Both targets exist only for `Message` owners. `plan` refuses `Head` owners, and any out-of-range part or index, with the same error style as binaries.
- A whole-row selection subsumes the row's fences and sources, as it does binaries.

**Mask:**
```rust
pub struct Mask {
    #[serde(default)] pub whole: bool,
    #[serde(default)] pub binaries: Vec<BinaryAt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub fences: Vec<FenceAt>, // {part, index}, sorted
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub sources: Vec<u32>,    // call part indexes, sorted
}
```
- `is_empty` considers all four. Masks already stored decode unchanged. No migration.
- **Bump `SESSIONS_WIRE_SCHEMA` 66 → 67.** The comment must say why: an older engine would ignore `fences` / `sources` and replay content the user removed, which is the invariant `Mask::decode` guards.
- `ContextRemoval` gains `surfaces: u32` (fences and sources newly excluded).

## 4. Projection (what the model sees)
**Fence:** replace the whole fence, from the opening line through the closing line, with one line. Text before and after is unchanged, and the surrounding newlines are kept.
- json-render: `[diagram removed from context: MetricGrid, Checklist]` (the label's types), or `[diagram removed from context]` when no types parse.
- mermaid: `[mermaid diagram removed from context]`.
- openui: `[openui surface removed from context]`.

**Source:** in that call's arguments, replace that one field's string with a marker:
- `[page removed from context: <name>]` for a `show` page or `page_preview`;
- `[app removed from context: <name>]` for `app`;
- `[file removed from context: <name>]` for any other `show`.

`<name>` = the `name` arg, else `title`, else the tool name. Every other argument, the call id and the paired ToolResponse stay untouched, so the exchange stays protocol-valid.

**Consumers:** every consumer of the effective frame sees the projection: the next request, compaction, harvest, `estimate()` and `ContextBreakdown`. These already go through `project`; confirm each one. The raw history keeps the originals.

## 5. Transcript metadata (`ContextEntry`): additive
```rust
pub struct ContextPiece { pub target: ContextTarget, pub label: String, pub removed: bool }
// ContextEntry gains (assistant rows only):
#[serde(default, skip_serializing_if = "Vec::is_empty")] pub fences: Vec<ContextPiece>,  // text order across the row's parts
#[serde(default, skip_serializing_if = "Vec::is_empty")] pub sources: Vec<ContextPiece>, // part order
```
Both are optional arrays in TS.

## 6. RPC
- `context_items` (existing) returns the extended snapshot.
- `remove_context(session, expected_revision, targets)` (existing) accepts `fence` and `source` targets.
- No new methods. Admission (idle native sessions only), the revision guard and no-op behaviour are unchanged.

## 7. `/shake` words (desktop slash and TUI)
`/shake [images|documents|surfaces|tools|notes|agents|messages|summary]`:
- `images`, `documents`, `surfaces`, `tools`, `notes` and `agents` open the picker with that whole category selected (`agents` means `peer`).
- `messages` opens it with the Conversation section open and nothing selected.
- `summary` selects the head, as today.
- An unknown word shows usage, as today.
- TUI: add the words and show `kind · group` in its list lines; no new TUI screens.

## 8. Desktop (per the mock)
**Picker** (`ContextPicker.tsx`, rewritten):
- Two sections: **Attachments & output** (`image`, `surface`, `tool`, `note`, `peer`, `document`, in that order) and **Conversation** (`user`, `assistant`, `summary`; shown as "Your messages", "My replies", "Summary").
- Category row: a tri-state checkbox (Attachments & output only), color dot, name, a line of group counts, the count (`n/N` when partly selected), a token bar with `~tokens`, and a chevron.
- Conversation categories have NO select-all: open one and tick rows (Sky's ruling).
- An opened category shows group rows (when there is more than one group), each with its own tri-state checkbox, count and tokens.
  - Tool groups: the 6 largest by tokens, then **Other tools** for the rest.
  - Items: checkbox, an image thumbnail when a preview path is available (else a format badge), label, detail and time, tokens.
  - Four items per group, then "Show N more".
- Footer: `N selected · ~T tokens · leaves ~R`, **Cancel**, and **Review removal…** (disabled while nothing is selected).

**Review** (regrouped):
- One line per category, or per group when the category is partly selected, with tokens; plus the companion count.
- Warnings:
  - a summary image or the head: "removing it loses part of the earlier conversation";
  - diagrams: "cut out of their replies; the reply text stays, with a one-line marker";
  - cards: "keep their card in your transcript; the model sees a one-line marker instead of their source".
- Then the 48 line about keeping originals with no undo, and **Remove from context** (danger).
- Commit and refresh follow 48 (toast, no optimistic splice).
- 44 px rows; keyboard (arrows, Space toggles, Enter is the footer default, Escape cancels); visible focus. One component serves desktop, iOS and browser.

**Transcript:**
- Each rendered surface fence in a reply gets a corner **Remove from context** control (44 px hit area, shown on hover, focus and coarse pointers). It appears only when the row's `fences` count equals the number of surface fences the renderer found, otherwise there is no control.
- A removed fence renders as a collapsed **Removed from context** marker with **View original**.
- show/app cards get **Remove source from context** in their header, from `sources`. Once removed, the card stays and shows a small "Source removed from context" tag.
- Busy, stale and external sessions keep controls disabled (48).

## 9. Not included
- Images or pages produced inside python cells (`await show(...)` in code). They are python tool exchanges: shake them under Tool output › python.
- Partial removal inside tool results (except images) or user messages.
- Automatic, threshold-based or age-based shaking, and any selector language.
- Mesh work beyond the wire bump; a TUI redesign.
