# 201 as built — P4 (artefacts and images)

D9, §8, F4–F7.

## Landed

- **`endless/units.rs` `derive`**: a binary part is `[image: <name>]` / `[document: <name>]` through `crate::loop_::binary_tag` (kind `talk` on an assistant row, else `user`). Each image of a `ToolResponse` is its own `echo` unit right after the response's text unit (part order, then image order). An empty / whitespace user text (an image-only message) is no unit. A user text starting with `VISION_MARKER` (`<vision_observation>`) is kind `note`.
- **Part encoding**: a tool image's unit `part` = `image_part(p, k)` = `IMAGE_PART (0x8000) | p << 7 | k`; `locate(part) -> (content part, Option<image>)`. Plain parts keep their index, so every unit row stored before P4 keeps its `part`, and `load`'s re-derivation (`find(|u| u.part == r.part)`) still matches.
- **Index stability**: units are persisted (`endless_units`) and `ingest` derives only rows after `live.last_seq`, so existing rows keep their stored units, kinds and indices; the new derivation applies only to rows ingested after P4. Old rows re-derive their *text* at `load` (an old `[image]` unit now reads `[image: name]`; its stored kind is kept, so an old vision note stays `user`). Old tool pictures get no unit (no stored row for them).
- **Vision notes**: `units::note_sources` parses the `Sources:` line (`m12.p1`, `m12.p3.i0`), `link_note` prefixes `[on #i, #j] ` with the image units it resolves; `pump::link_notes` runs it at `ingest` (against new and live units) and at `load`. `note_answer(text, seq, part, image)` reads one image's answer (none when `NOT SEEN`).
- **Meetings**: no switch needed — every changed or new unit (`[image: …]` user, tool-picture echo, `note`, the dropped empty caption) is one `UnitFilter::Meeting` drops anyway (it keeps only `[meeting…` user units and meeting tool calls). `describe_left` returns early for meeting trees (`!drafting()`).
- **Descriptions (`pump::describe_left`, `Endless::describe_left`)**: called in `loop_.rs` `endless_tail` right after a trim's `start_generation` with the rows `[old first_seq, new first_seq)` and `self.vision_model()`. Not awaited: a `tokio::spawn`ed task (returns the handle for tests). Off when `describe_images: off`, epochs, or meeting. For each `[image: …]` unit in range whose l0 node has no `described:` model: a vision note's answer if one exists (`described:note`), else `Compactor::describe_image` on the compactor when `accepts_images()` (catalog `supports_images`; `described:compactor:<id>`), else on a `model_compactor` for the design-52 vision model (`described:vision:<id>`), else nothing. The binary is re-read with `Store::endless_message(seq)`. The reply's first non-empty line is appended as ` — <desc>`, the whole cut to `node_bytes` (`described`); the node stays `free`, `model` records the source; stored with `endless_nodes_put`, then `live.put` (draft ancestors rebuild). The generation frozen at the trim shows the line as it was; the next generation shows the description.
- **Compactor**: trait `accepts_images()` (default `backend::supports_images(model)`) and `describe_image(&Binary)` (default: error, sends nothing). `ModelCompactor::describe_inner`: `DESCRIBE` system prompt, the image in one user message, reply cap `SMALL_REPLY_TOKENS`; the streaming half of `compress_inner` moved to `ModelCompactor::send` (bytes unchanged). Guards are not applied beyond size (`node_bytes`) and non-empty.
- **Zoom (`loop_/endless_tools.rs`)**: `zoom(id, 1)` on an artefact unit (`Endless::artefact` → `pump::artefact`) returns `id+1|<line>` plus the binary written to `<tmp>/theuth-zoom/<session>/<id>-<name>` in `ToolOutput.files`, which `finish_batch` attaches as a tool image like any picture tool. Row/part gone: the line plus "(the file itself is no longer stored)". `ViewLine` (wire type) unchanged.
- **Token budget**: `tail::bytes` = `compact::est` × 4, and `compact::est` charges `IMAGE_TOKENS` per image part and per tool-response image (P2; existing tests `est_charges_images_flat_and_pdfs_per_page`, `est_charges_tool_response_images_flat`). Checked, no change.

## Deviations

- The description job is a plain spawned task, not a pump slot: it does not take the local `Gate` (one call per image leaving the tail). A local vision compactor could therefore overlap a refinement call.
- Only `[image: …]` units are described; documents (PDF) are not (§8 says "an image").
- The vision-model describer is built with `model_compactor(…, agent "theuth", effort None)`; its spend is not recorded in the cost ledger (the compactor's describe path skips `Tally`).
- Zoom writes the binary to a temp file (the tool contract carries paths, not bytes); files are not cleaned up.

## Tests

`endless::units::tests::{binary_shapes_yield_named_units, vision_note_is_a_linked_note}`, `endless::tests::{trim_describes_images_leaving_the_tail, no_description_when_off_or_text_only, zoom_on_an_image_unit_returns_the_binary}`.

## Not done

- No dedicated test that indices of a pre-P4 session do not shift (argued above from `ingest`/`load`; not exercised with hand-written pre-P4 unit rows).
- The vision-model path (`described:vision:`) is not covered by a test; the gate "a screenshot is recoverable by `zoom` two generations later" is covered only at the pump level (artefact reads the row regardless of generation).
