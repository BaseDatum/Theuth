# 52 notes — read_file shows images; image input never breaks a session

Status: **as-built · 2026-11-11**. Extends [52](52-vision-fallback.md). Sky's ask
(2026-11-11): `read_file` on an image must give the agent the picture, and a
model that cannot take images must never break the session: catch the error,
drop the image from what is sent, put text in its place (the vision fallback's
description when one is configured, else a note).

## 1. Images in

| Source | Path |
|---|---|
| `read_file` | Magic bytes (PNG, JPEG, GIF, WebP) → `tools::images::tool_output`: decoded, scaled to a long side of `MAX_EDGE` (1568, `computer_screenshot`'s `max_edge` default) and to at most `MAX_IMAGE_BYTES` (5 MB, the attachment cap), saved content-addressed under `<resources>/images/<session>`, attached as `ToolOutput.files`. Text part: one header line `[read_file <path>: image/png 1600×1100, 12 KB]`, plus `; scaled to W×H` when downscaled. |
| `read_url` | The page fetch refuses `image/*` before the body (and now refuses image magic bytes under any other label); `read_url` then does one raw GET through `Fetcher::request` (same SSRF guard, `max_fetch_bytes` cap) and returns the picture the same way. |
| Every attachment and tool file | `loop_::file_part` → `tools::images::for_provider`: MIME from the bytes (genai's `Binary::from_file` took it from the extension), decoded, scaled into `PROVIDER_MAX_SIDE` (8000 px) and 5 MB. An image the guard refuses, or a name that says image over bytes that are not a supported one, becomes the existing `[file … unreadable: …]` note. |

Decisions:

- `offset`/`limit`/`tail`/`ranges`/`outline` on an image are **refused** with a
  message naming them; `numbers` is ignored.
- BMP, TIFF and HEIC are not added: the workspace `image` crate decodes only
  png/jpeg/gif/webp. Such a file reads as text bytes in `read_file` as before;
  as an attachment or tool file it is refused at ingress with a note.
- `read_url` images over `max_fetch_bytes` (2 MiB default) are a text error.
  Ceiling: raise the cap for image bodies if large web images matter.
- The size guard runs at ingress, once per image. Images stored in history
  before this change are sent as stored.

Every image source reaches the request through one of the two ingress points
above and then through 52's `Turn::prepare_images`: user attachments
(`user_message`), and every tool's `ToolOutput.files` in `run_batch` —
`read_file`, `read_url`, `artifact_read`, `browser_screenshot`,
`computer_screenshot`, `computer_clipboard_read(format=image)`, MCP image
blocks, and the outer result of a python cell (52 §7.1 snapshots). None
bypasses it.

## 2. A provider that refuses images

52 §4.1 says the fallback is not provider-error recovery. Sky's ask overrides
that for one narrow class of error; nothing else changes.

- **Classifier** `vision::rejects_images`: a client status token (`400`,
  `415`, `422`) AND an image word (image, vision, multimodal) AND a refusal
  phrase ("not supported", "unsupported", "only supported", "unknown variant",
  "at most 0 image", …). Table test over OpenAI, OpenAI-compatible, DeepSeek,
  vLLM, Anthropic, Bedrock, Groq and LiteLLM bodies; size ("image exceeds
  5 MB"), dimension, "Invalid image", 429, 5xx and overflow bodies do not
  match. One classifier for all backends: genai renders every backend's
  status and body into the error string.
- **On a match** in a round call (`Turn::round`, `Abort::Model`), when the
  model was sent images: the model id joins `Shared::image_refusals` and the
  store's local `image_refusals` table (migration `0058`), `Current::vision()` is false
  for it from then on, and the round returns `Round::Retry`. The retry runs
  52's preparation: fallback description when `agent.vision_model` is set,
  else the notice (now naming the refusal). A marked model is never sent an
  image again, so the retry happens once.
- **Durability: reapplied from the remembered flag.** The flag is loaded from
  the store once per session at its first round; 52's projection is a request
  copy, so the stored transcript keeps its images and stays consistent, and a
  reload, resume or restart sends that model text. The table is local, not
  attached to the session log: `SESSIONS_WIRE_SCHEMA` stays 53 so session
  sync between mixed versions is not held for this. A fork or a replica does
  not carry the flag and re-learns it with one refused call. Deleting the
  session deletes its rows.
- **Retry fails too:** the ordinary round error (`model: …`), the turn ends,
  the session takes the next message.
- **Switch to a text-only model with images in history:** 52 §7.2, unchanged;
  now covered by a test.
- Only round calls are classified. A summary/harvest continuation refused for
  images reports its error as before; the next round marks the model.

## 3. The fallback itself

Already 52: an erroring, timing-out or unset fallback yields a `NOT SEEN`
observation or the notice and the turn continues; a failed occurrence is not
retried within the turn. The vision call's timeout is `VISION_TIMEOUT`
(= `SUMMARISE_TIMEOUT`, 300 s; 3 s under test).

Caching stays per occurrence (source coordinates + content fingerprint), as
52 built it: one consultation per image occurrence, never repeated once
answered. Reading the same file twice is a second occurrence and is consulted
again: 52 §5.3 "reattaching an image creates a new occurrence and
consultation", kept. No content-hash dedupe across occurrences was added.

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
