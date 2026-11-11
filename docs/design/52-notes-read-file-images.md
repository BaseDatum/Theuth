# 52 notes — read_file shows images; image input never breaks a session

Status: **as-built · 2026-11-11** (review round 1 applied). Extends
[52](52-vision-fallback.md). Sky's ask (2026-11-11): `read_file` on an image
must give the agent the picture, and a model that cannot take images must
never break the session: catch the error, drop the image from what is sent,
put text in its place (the vision fallback's description when one is
configured, else a note).

## 1. Images in

| Source | Path |
|---|---|
| `read_file` | Magic bytes (PNG, JPEG, GIF, WebP) → `tools::images::tool_output`: `fit` to a long side of `MAX_EDGE` (1568, `computer_screenshot`'s `max_edge` default) and at most `SEND_MAX_BYTES`, saved content-addressed under `<resources>/images/<session>`, attached as `ToolOutput.files`. Text part: one header line `[read_file <path>: image/png 1600×1100, 12 KB]`, plus `; scaled to W×H` when downscaled or `; re-encoded as <mime>`. A known format no provider takes (BMP, TIFF, ICO, HEIC, AVIF) is a one-line refusal: "BMP images aren't supported …; convert it to PNG". |
| `read_url` | The page fetch refuses `image/*` before the body (and image magic bytes under any other label); `read_url` then does one raw GET through `Fetcher::request` (same SSRF guard, `max_fetch_bytes` cap) and returns the picture the same way. Non-2xx, cut or undecodable: a text error. |
| Every attachment and tool file | `loop_::file_part` on a blocking thread → `tools::images::for_provider`: MIME from the bytes (genai's `Binary::from_file` took it from the extension), decoded, `fit` to `PROVIDER_MAX_SIDE` (8000 px) and `SEND_MAX_BYTES`. An image the guard refuses, a decoder panic, or a name that says image over bytes that are not a supported one becomes the existing `[file … unreadable: …]` note. |

`fit` (`tools::images`): decoded under side (20 000 px) and allocation (256 MiB)
limits; EXIF orientation applied; a still PNG/JPEG that fits goes unchanged,
anything else is re-encoded — GIF and WebP always, as a still PNG of the first
frame (OpenAI takes only still GIFs), a rotated photo upright, a large one
scaled (JPEG stays JPEG, else PNG, then JPEG with alpha flattened onto white,
then halved while over the cap).

Decisions:

- `SEND_MAX_BYTES` is 3.75 MB, Bedrock's limit and the lowest common one, for
  every provider. `MAX_IMAGE_BYTES` (5 MB) stays the ingress acceptance cap
  (MCP blocks, python snapshots); anything between is re-encoded at
  `file_part`.
- `offset`/`limit`/`tail`/`ranges`/`outline` on an image are **refused** with a
  message naming them; `numbers` is ignored.
- The size guard runs at ingress, once per image. Images stored in history
  before this change are sent as stored.
- Not done (NITs): `read_url`'s image cap stays `max_fetch_bytes` (2 MiB), and
  an octet-stream image is downloaded twice (page probe, then raw GET).

Every image source reaches the request through one of the two ingress points
above and then through 52's `Turn::prepare_images`: user attachments
(`user_message`), and every tool's `ToolOutput.files` in `run_batch` —
`read_file`, `read_url`, `artifact_read`, `browser_screenshot`,
`computer_screenshot`, `computer_clipboard_read(format=image)`, MCP image
blocks, and the outer result of a python cell (52 §7.1 snapshots).

## 2. Errors the images cause

52 §4.1 says the fallback is not provider-error recovery. Sky's ask overrides
that for errors the images cause; nothing else changes.

**Classifiers** (`vision`), with table tests over OpenAI, OpenAI-compatible
(genai's text captured from the scratch engine), DeepSeek, vLLM, Anthropic,
Bedrock, Groq, LiteLLM, llama.cpp, Ollama and an SSE error event:

- `rejects_images` — the model takes no images. A *strong* phrase ("image
  input is not supported", "does not support image", "missing data required
  for image", "not a multimodal model", "unknown variant image_url",
  "image_url is only supported", "at most 0 image", …) matches with any
  status or none: llama.cpp and Ollama answer 500, a stream error event has no
  status. `compact::classify` calls a strong refusal deterministic, so it is
  never resent with backoff. A *weak* phrase ("not supported", "incompatible",
  "unsupported media type", …) needs a 400/415/422 and an image term
  ("image input", "images", "image/…", "vision", "multimodal", …) in the prose;
  identifier-like words (`generate_image`) are ignored. Bare "unsupported" is
  not a refusal: "unsupported image" is one image's defect.
- `image_caused` — a 400/413/415/422 naming an image that is not a refusal:
  too large, too many ("At most 20 image(s)"), unreadable, content policy.
  Overflow is excluded (compaction owns it).

**Round** (`Turn::image_failure`), only when the frame holds images:

- Refusal → the model is marked text-only for the session (`Shared::
  image_refusals` + the local `image_refusals` table, migration `0058`), one
  notice row says "<model>'s provider refused an image (<reason>). This chat
  now sends <model> text in place of images … Re-select the model to send it
  images again.", and the round retries once through 52 (fallback
  description, else the notice).
- Undo: **re-picking the model** (`set_model` → `SwitchModel`) clears its mark.
- The retry refused the same way with no image in it → the refusal was not
  about images: the mark is cleared, a notice says so, and the turn ends with
  the ordinary `model: …` error. The next message sends images again.
- `image_caused` → the images go through 52 for the rest of this turn only,
  one retry, the model not marked. The image stays in history, so a later turn
  sends it natively again and, if refused again, retries again once.
- Retry fails otherwise: the ordinary round error; the session takes the next
  message.

**Compaction and harvest**: `resolve()` loads the flag too, so `/compact` and
`/knowledge harvest` in a fresh process never send a marked model images. A
summariser or harvest continuation refused for its images (either class) marks
the model on a refusal and resends once with the images stripped.

**Durability**: reapplied from the remembered flag; 52's projection is a
request copy, so the stored transcript keeps its images. The table is local
(not attached to the session log): `SESSIONS_WIRE_SCHEMA` stays 53. A fork
copies the session's marks; a replica re-learns with one refused call;
deleting the session deletes its rows.

**Switch to a text-only model with images in history**: 52 §7.2, unchanged.

## 3. The fallback itself

- An erroring, timing-out or unset fallback yields a `NOT SEEN` observation or
  the notice and the turn continues (52).
- Each auxiliary call (question, vision) is bounded by `AUX_TIMEOUT` = 75 s
  (was the summariser's 300 s).
- A fallback whose vision call fails or times out is not asked again in this
  session for `FALLBACK_COOLDOWN` (10 min); one that refuses images is not
  asked again in this session at all. Meanwhile images get the notice naming
  why. A malformed reply (the call worked) does not start a cooldown; the next
  user turn retries it once, as 52 built.
- **Each distinct image is described once per session**: `vision::pending`
  matches answered observations by content fingerprint, so the same bytes
  (the same file read twice, the same screenshot) reuse the first answer; both
  markers carry the same `fp`. This replaces 52 §5.3's "reattaching an image
  creates a new occurrence and consultation" at Sky's rule.

## 4. Prompting

- The runtime index shows a tool's first sentence cut at 120 chars
  (`repl::summary`). `read_file`'s first sentence (119 chars) and
  `read_url`'s (85) both say they return images as pictures.
- The standing guidance (`frame::GUIDANCE_TAIL`, part of every native
  session's baseline, root and theuth subagent alike) says: to look at an
  image, `read_file(path)` or `read_url(url)` puts the picture in context;
  a text-only model gets the fallback's description or a note.
- `browser_screenshot` and `artifact_read` name the same text-only
  behaviour. `repl::tests::index_lines_and_guidance_name_images` checks the
  index lines are uncut and mention images, and that the guidance has the
  sentence.

## 5. Merge note

`0058_image_refusals.sql` is the next free migration on main as of
2026-11-11; renumber it if another branch lands a `0058` first.
