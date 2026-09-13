# 17 — Images: sending them to models, pasting them in the TUI

Status: as-built v0 · 2026-09-12 (draft v0 same day) · extends `07-context.md`
(frame, `est`), `08-repl.md`/TUI input, `06-acp.md` §4 (wth as agent),
`09-signal.md` §7.4. §9 lists what the build changed against the draft; where
§9 disagrees with §1–§8, §9 wins.

Verified 2026-09-12 against a generated 400×300 PNG (red square left, blue
circle right). ACP (`wth --acp`, throwaway Bun client): `initialize` →
`promptCapabilities {image: true, audio: false, embeddedContext: true}`;
`session/prompt` with a text block and an `image` block, parent
`anthropic/claude-sonnet-4-5` → `~/.wth/attachments/0aaee72171ac58d9.png`
written, `messages.body` row holds `[Text, Binary{image/png, Base64}]`
(2040 B), reply `A red square and a blue circle.`, `end_turn`; `session/load`
replayed `user_message_chunk` ending in `[image: 0aaee72171ac58d9.png]`.
Same prompt with `WTH_AGENT__MODEL=cursor/gpt-5` → `I wasn't given the
image—only the note "[image omitted: model is text-only]".` TUI on a pty
(`osascript` set the clipboard from the PNG): `\x16` → `[⎘ image 1: 400×300]`
in the input; bracketed paste of `'/tmp/wth-img/shapes.png'` → `[⎘ image 2:
shapes.png]`; question + Enter → transcript kept the tokens, reply from
`openai/gpt-5.6-luna`: `Both images show a red square and a blue circle.`
Unit: `est` flat per image, `Frame::request(vision=false)` strips,
`check_files` boundaries, `expand_pastes` drops deleted tokens, `downscale`
4×2→2×1, ACP image block → sha-named file, Signal PNG path in `files`.
Not exercised: the 5 MB rejection live (unit only), Windows/X11/Wayland
clipboards, Gemini/OpenAI-Responses image bodies.

## 0. What already exists

Most of the model side is built; snap compaction (13) needed it.

| Layer | State today | Cite |
|---|---|---|
| History element | genai `ChatMessage { content: MessageContent(Vec<ContentPart>) }`; `ContentPart::Binary(Binary { content_type, source: Base64(Arc<str>) \| Url, name })` | genai `chat/binary.rs`, `message_content.rs` |
| Multipart user message | built for snap heads: `ContentPart::from_binary_base64("image/png", …, Some("frame-N.png"))` inside `ChatMessage::user(MessageContent::from_parts(parts))` | `agent/src/loop_.rs:1531-1548` |
| Provider bodies | every genai adapter maps a user-role `Binary` itself: Anthropic `{"type":"image","source":{"type":"base64",…}}`, OpenAI chat `image_url` data-URL, Responses `input_image`, Gemini `inline_data`, Bedrock, Ollama. wth builds no JSON. | genai `adapter/adapters/*` |
| Persistence | `messages.body TEXT` = whole `ChatMessage` as JSON; `Binary` parts round-trip (test `snap_head_round_trips_parts`) | `agent/src/store.rs:191-205, 669-711` |
| Frame | `Frame::request` copies history verbatim — content-agnostic | `agent/src/frame.rs:68-112` |
| Vision flag | `models::Model.image` from models.dev `modalities.input` → `ModelInfo.image`; gate `vision = Genai && model.image` | `agent/src/loop_.rs:1421-1424` |
| PNG encode | `png` 0.17, `snap::encode` (grayscale) | `agent/src/snap.rs:214-223` |
| Bracketed paste | iocraft fork forwards `TerminalEvent::Paste`; large pastes become `[⎘ paste N: …]` tokens held in `Model::pastes`, expanded at submit | `tui/src/ui.rs:140-150`, `tui/src/model.rs:431-439, 986-1003` |

What is missing is one thing, repeated at every hop: the user turn is a
`String`. `send_input(session, text)` → `SendInput{text}` → `UserInput{text}`
→ `ChatMessage::user(text)`. Nothing between the keyboard and the model can
carry an image. Readers that flatten history (`text_of`, `HistoryEntry::User`,
`acp::render`, `rpc/acp.rs::prompt_text`) drop or placeholder binary parts.

## 1. Facts relied on

- Bracketed paste carries text only. A terminal's own paste key (Cmd+V,
  Ctrl+Shift+V) with an image on the clipboard delivers nothing to the app.
  Reading the image needs the OS clipboard API: `arboard` 3.6 (1Password),
  `Clipboard::get_image() -> ImageData { width, height, bytes: RGBA8 }`,
  macOS/Windows/X11/Wayland. Its `image-data` feature pulls `image` 0.25 with
  `default-features = false` (tiff on macOS, png elsewhere); we do not call
  `image` — we already have `png`.
- Dropping a file onto a terminal pastes its path as text (ghostty, iTerm2,
  kitty, Windows Terminal), sometimes shell-quoted.
- Ctrl+V reaches the application on macOS and Linux terminals (Cmd+V /
  Ctrl+Shift+V are the terminal's). Windows Terminal binds Ctrl+V to paste by
  default; there, drag-drop is the path.
- Provider limits: Anthropic 5 MB and 8000 px per image, bills ≈ w×h/750
  tokens, recommends ≤ 1568 px long edge (larger is downscaled server-side).
  OpenAI 20 MB. Gemini 20 MB per request inline. 5 MB is the binding cap.
- A 2× retina screenshot (2880×1800) is 1–6 MB as PNG; halved (1440×900) it
  is 4–9× smaller and loses nothing the provider would keep.
- `compact::est` = serialised JSON chars / 4 (`compact.rs:40-43`): a 1 MB PNG
  counts as ~340k tokens against a real bill of ~2k. It feeds the
  auto-compaction trigger (`loop_.rs:1043-1052`) and the kept-tail budget
  (`keep_from`), so one screenshot would trip compaction and evict the tail.
- The socket RPC is a Unix socket; every client is on this machine and can
  read the same files.

## 2. Shape: one field, paths

Every input carries `files: Vec<PathBuf>` next to `text`. Paths, not bytes.

```rust
// agent/src/lib.rs
pub struct SendInput { pub session: Option<SessionId>, pub text: String, pub files: Vec<PathBuf> }
pub struct Steer     { pub session: SessionId, pub id: InputId, pub text: String, pub files: Vec<PathBuf> }
pub struct QueuedInput { pub id: InputId, pub text: String, pub files: Vec<PathBuf> }
// agent/src/loop_.rs
pub struct UserInput { pub id: Option<InputId>, pub text: String, pub files: Vec<PathBuf> }
type Queued = (Option<InputId>, Input);            // Input = { text, files }
// rpc/src/api.rs
async fn send_input(&self, session: Option<SessionId>, text: String, files: Vec<PathBuf>) -> RpcResult<SessionId>;
async fn steer(&self, session: SessionId, text: String, id: Option<InputId>, files: Vec<PathBuf>) -> RpcResult<InputId>;
// tui/src/model.rs
Command::Send  { session, text, files }
Command::Steer { session, id, text, files }
```

`files` is `#[serde(default)]` so today's `send_input(session, text)`
callers keep working. Steer carries it because a paste while the agent is
running goes down the steer path (`model.rs:1814-1834`); without it the
image would be lost exactly when the user is most likely to send one.

Why paths: every producer already has or trivially makes one (TUI writes
the clipboard PNG to disk to name it; Signal already writes attachments to
`<data_dir>/signal/inbox/`; ACP decodes base64 once and writes). RPC frames
stay small (jsonrpsee default 10 MB request cap would bite base64). The agent
turns a path into a part with one existing call, `genai::Binary::from_file`
(mime_guess + base64 + name). The sqlite row is the durable copy; the file is
a transport handle and a thing the user can `@`-reference or re-attach.

Rejected: base64 in the RPC/actor message (`Vec<u8>` serialises as a JSON int
array — 4× bloat — or needs a `serde_with` shim; and the TUI would still
write the file to give the token a name); a new `Attachment { mime, bytes }`
contract type in `runtime` (00 §4 caps that crate; a `PathBuf` is not a new
type); `Vec<ContentPart>` over RPC (leaks genai into the wire API).

## 3. Agent

### 3.1 Validation, once

All producers funnel through `Agents::input` (`supervisor.rs:389-413`), whose
errors already surface as `AgentEvent::Error`. Add there:

```rust
/// Rejects anything a provider would: not a regular file, not image/*, over 5 MB.
fn check_files(files: &[PathBuf]) -> Result<(), String>
```

`image/*` via `mime_guess::from_path` (genai dependency, already in the
tree; add it to `agent/Cargo.toml`). On failure the whole input is dropped
with `Error { message: "attachment shot.png: 7.2 MB exceeds 5 MB" }` — the
same outcome as a bad session id today. The TUI pre-checks extension and size
at attach time (§4) so this path is for RPC/ACP callers, the trust boundary.

### 3.2 The message

`Turn::run` (`loop_.rs:935-939`) and the steer injection (`965-966`):

```rust
fn user_message(text: String, files: &[PathBuf]) -> ChatMessage {
    if files.is_empty() { return ChatMessage::user(text); }
    let mut parts = Vec::with_capacity(files.len() + 1);
    parts.push(ContentPart::Text(text));
    for f in files {
        parts.push(match Binary::from_file(f) {
            Ok(b) => ContentPart::Binary(b),
            Err(e) => ContentPart::Text(format!("[image {} unreadable: {e}]", f.display())),
        });
    }
    ChatMessage::user(MessageContent::from_parts(parts))
}
```

Text first, then images, matching the snap head. `from_file` failing here
means the file vanished between check and read; a placeholder beats
aborting the turn. `input_text` (→ `TurnStarted.text`) stays text-only.

### 3.3 Text-only models: strip at the frame

`Frame` gains `vision: bool` (the existing expression at `loop_.rs:1423`,
hoisted to `Turn::vision()`). In `request`, when `!vision`, every user-role
`Binary` part in `head` and `history` becomes
`Text("[image omitted: model is text-only]")`. This is the only place it
needs doing:

- Cursor serialises text parts only (`cursor/src/history.rs:445-459`) —
  same result, but now the model is *told* an image was there.
- OpenAI/Gemini text-only models 400 on image parts; Anthropic non-vision
  models likewise. Stripping avoids the round-trip.
- Gateway models (`providers.rs:203`) are written `image: false`; at
  `Catalog::load` a zero-limit model inherits `image` (and limits, reasoning)
  from its closest models.dev sibling by name, so a vision model behind a
  named gateway is only stripped when no sibling exists. Override: edit the
  model in `<data_dir>/models.json`.
- `/model` from a vision model to a text-only one and back: history keeps
  the bytes, the frame decides per call. Nothing is lost.
- Summary compaction (`loop_.rs:1480-1483`) builds its own request; it uses
  the same `frame::strip_images(&mut ChatMessage)`.

No new `AgentEvent` for "image stripped": the placeholder is what the model
sees, and the user is told before Enter by the TUI pre-check (§4.4).

### 3.4 `est`

```rust
/// Serialised chars / 4, except a binary part counts a flat IMAGE_TOKENS (1600):
/// providers bill images by pixels, and a ≤1568 px PNG lands near that.
pub fn est(m: &ChatMessage) -> u32
```

Iterate `m.content` parts: `Text` → `len/4`; `Binary` → 1600; others →
serialised `len/4` as today. Fixes the trigger and `keep_from` for pasted
images and for snap heads alike (13 measured ~4.5k for a full 1568² frame;
1600 for a typical screenshot is the right order and errs toward compacting
later, which the real `usage` corrects on the next call).

### 3.5 Readers

`text_of` (`loop_.rs:1730`, `supervisor.rs:1654`): `Binary(b)` →
`"[image: {name}]"`. `HistoryEntry::User { text }` then shows the marker on
`load_session`/`--resume` with no type change. `snap::serialise` already
writes `[image frame omitted]`; unchanged.

Store: no migration. A session with 30 screenshots is a 30–60 MB read on
resume; acceptable. `ponytail:` upgrade is rewriting stored `Base64` to
`Url("file://…")` at persist time and rehydrating at frame build.

## 4. TUI

`model.rs` stays pure; clipboard, files and PNG live in `ui.rs` (the I/O
layer), with `rt.data_dir` passed into `App` like `api` is.

### 4.1 Ctrl+V: clipboard image

In `use_terminal_events` (`ui.rs:153-199`), before the menu check:

```rust
if code == KeyCode::Char('v') && modifiers.contains(KeyModifiers::CONTROL) && m.input_focused() {
    match clip::image(&data_dir) {            // arboard → downscale → png → file
        Ok(Some(img)) => insert_at_cursor(&m.attach(img)),
        Ok(None) => {}                        // no image on the clipboard: silent
        Err(e) => m.entries.push(Entry::Error(format!("paste: {e}"))),
    }
    return;
}
```

`tui/src/clip.rs`:

```rust
pub struct Image { pub path: PathBuf, pub w: u32, pub h: u32 }

/// The clipboard image as a PNG under `<data_dir>/attachments/<sha256[..16]>.png`,
/// long edge ≤ 1568 px (integer box downscale; 2× retina → exactly half).
/// `None` when the clipboard holds no image.
pub fn image(data_dir: &Path) -> Result<Option<Image>>
```

`arboard::Clipboard::new()?.get_image()`: `ContentNotAvailable` → `None`;
other errors → `Err`. RGBA8 → factor `f = ceil(max(w,h)/1568)`, average each
`f×f` block (`ponytail:` integer factor only; a proper resampler needs
`image`, which 13 §0.4 chose not to depend on). Encode with `png` as
`Rgba`/`Eight` (the `snap::encode` shape, one colour type different — put
the encoder in `tui`, not shared; two 10-line functions beat a crate
boundary). Name by sha256 of the PNG so re-pasting the same shot is one
file. Blocking I/O on the event thread: a clipboard read plus one
`fs::write` of ≤ 1 MB, milliseconds; fine.

Text-only clipboard: `get_image()` returns `ContentNotAvailable` → `None` →
nothing happens, which is also what plain Ctrl+V does today.

### 4.2 Paste of an image path (drag-drop, copy-path)

`Model::paste` (`model.rs:986`) first checks: text is one line, unquote
`'…'`/`"…"`/backslash-escaped spaces, `Path::is_file()`, extension in
`png jpg jpeg gif webp`, size ≤ 5 MB → `self.attach(Image{path, w: 0, h: 0})`
(dimensions unknown without decoding; token shows the file name instead).
Otherwise today's paste behaviour. The file is used in place, not copied.

### 4.3 Tokens

`Model::pastes: Vec<String>` becomes `Vec<Paste>`:

```rust
enum Paste { Text(String), Image(Image) }
fn paste_token(i: usize, p: &Paste) -> String   // "[⎘ paste 1: 69 lines]" | "[⎘ image 2: 1440×900]" | "[⎘ image 2: shot.png]"
pub fn attach(&mut self, img: Image) -> String   // pushes, returns the token
fn expand_pastes(&mut self, line: &str) -> (String, Vec<PathBuf>)
```

One index space, so tokens read in paste order. `expand_pastes` replaces
text tokens with their text, removes image tokens from the text and collects
their paths; a token the user deleted from the input drops its image, the
same rule text pastes have. `submit` passes `files` to `Command::Send` /
`Command::Steer`; `Entry::User(line)` keeps the token form, so the
transcript shows `[⎘ image 1: 1440×900]` inline — no new `Entry` variant, no
terminal graphics.

### 4.4 Pre-check

`attach` on a text-only shown model (`ModelInfo.image == false`, already in
the model for the `· vision` label at `ui.rs:319`) still attaches but pushes
`Entry::Info("<model> is text-only; the image will be described as omitted —
/model to switch")`. The user decides before Enter.

### 4.5 `lib.rs`

`Command::Send { session, text, files }` → `api.send_input(session, text,
files)`; same for `Steer`. Two lines.

## 5. Other producers

- **ACP inbound** (`rpc/src/acp.rs`): advertise
  `PromptCapabilities::new().embedded_context(true).image(true)`. `prompt_text`
  becomes `prompt_parts(&[ContentBlock]) -> (String, Vec<PathBuf>)`:
  `ContentBlock::Image(img)` → base64-decode `img.data`, write
  `<data_dir>/attachments/<sha256[..16]>.<ext from mime_type>`, push the path;
  text handling unchanged. Zed/JetBrains pasting an image into their wth
  panel now works.
- **Signal** (`signal/src/channel.rs:956-983`): `save_attachments` already
  writes the file and emits `[attachment: path (mime, size)]`. Pass image
  paths in `SendInput.files` too, keep the line (the model still learns the
  path). One `Vec` threaded through `handle_envelope`.
- **ACP children** (`acp/src/harness.rs:351`, `acp_child.rs`): unchanged.
  Their inputs come from `spawn_subagent`/`send_message`, which are text
  tools; the model has no image to hand over. Upgrade when a tool wants it:
  `prompt(text, files)` → `ContentBlock::Image` when
  `init.agent_capabilities.prompt_capabilities.image`.
- **Cursor**: §3.3 strip; nothing in `crates/cursor` changes.

## 6. Not in v0

- **The terminal's own paste key (⌘V, Ctrl+Shift+V, right-click → Paste).**
  Every shipping terminal services it by reading the clipboard *as text*
  and writing that to the pty; a bitmap-only clipboard yields zero bytes, so
  there is no event for a TUI to hook (ghostty 1.3.1 `paste_from_clipboard`
  → `completeClipboardPaste(clipboardTextContent(..) orelse "")`; iTerm2,
  WezTerm, Terminal.app, Windows Terminal behave the same). That is why
  Claude Code, Codex CLI and this doc bind Ctrl+V + an OS clipboard read.
  The fix exists upstream: kitty ≥ 0.44.1 and ghostty main (since
  2026-08-21, not in 1.3.1) implement **mode 5522 paste events** from the
  kitty clipboard protocol — with `CSI ? 5522 h` set, the paste key writes
  an OSC 5522 `type=read:status=DATA:mime=.` listing of the clipboard's
  MIME types plus a one-time `pw`, and the app answers with
  `OSC 5522;type=read:pw=…:name=<b64 "Paste event">;<b64 "image/png">` to
  receive base64 chunks (≤ 4096 B raw each) — no permission prompt, works
  over SSH. Support is probeable with `CSI ? 5522 $ p` (DECRPM `Ps` 0/4 =
  unsupported). Cost: crossterm's parser does not know OSC 5522, so the
  response (megabytes of base64) has to be captured before iocraft's event
  loop sees it — a second patch on the iocraft/crossterm backend. Build it
  when a supported terminal is in daily use; until then Ctrl+V.
- Inline preview in the transcript (kitty/sixel/iTerm2 graphics). iocraft
  has no image primitive; the token names the file. Add when a second
  frontend wants it — Zed renders ACP images itself.
- `@path` mentions typed by hand, `/attach`. Drag-drop and Ctrl+V cover the
  two real gestures; typing a path is a paste of that path.
- PDF / audio. genai maps `application/pdf` to Anthropic `document` /
  OpenAI `file` already; the `image/*` check is the only thing excluding it.
  Widen when asked.
- Resizing files from disk (needs a decoder → `image` crate). Over 5 MB is
  rejected with the size in the message; clipboard pastes are downscaled.
- Attachment GC. `<data_dir>/attachments/` grows like `signal/inbox/` does.
- A `read_file` tool returning images to the model. Different feature (tool
  result parts), different doc.

## 7. Dependencies

`arboard = { version = "3.6", default-features = false, features = ["image-data"] }`
in `tui/Cargo.toml` (workspace-pinned in the root table with the reason: OS
clipboard image read; bracketed paste is text-only). `mime_guess` in
`agent/Cargo.toml` (already resolved via genai). `png` and `base64` are
present. Nothing else.

## 8. Build order and verification

1. `agent`: `files` on `SendInput`/`Steer`/`QueuedInput`/`UserInput`,
   `check_files`, `user_message`, `Frame.vision` + `strip_images`, `est`,
   `text_of`. Tests: `est` of a message with one `Binary` part ≈ 1600
   regardless of payload size; `Frame::request(vision=false)` yields no
   `Binary` part and one placeholder per image; `store` round-trip already
   covered.
2. `rpc`: `send_input`/`steer` signatures; ACP `image(true)` +
   `prompt_parts`.
3. `tui`: `clip.rs`, `Paste`, `attach`, Ctrl+V, path paste, `lib.rs`.
4. `signal`: thread paths.

Smoke, headless: `send_input(None, "what does this show?",
["/tmp/shot.png"])` against claude-sonnet-4-5 → `TextDelta` describing the
image; same against a Cursor model → answer mentions an omitted image;
`load_session` → `HistoryEntry::User { text: "…[image: shot.png]" }`;
`send_input` with a 7 MB PNG → `AgentEvent::Error` naming the cap.
TUI on a pty (the `tui_smoke.py` harness): `osascript -e 'set the clipboard
to (read (POSIX file "/tmp/shot.png") as «class PNGf»)'`, send `\x16`
(Ctrl+V) → input shows `[⎘ image 1: W×H]`; type a question, Enter → the
transcript keeps the token and the reply describes the image. Drag-drop
simulated by pasting `'/tmp/my shot.png'` → same token with the file name.
ACP: the throwaway Bun client from 06 sends `session/prompt` with an
`image` block → same reply path.

## 9. As-built deltas

- **`QueuedInput` unchanged** (`{id, text}`). It is a display type with a
  ts-rs export; the loop's internal `Queued` carries the files as
  `(Option<InputId>, Input)` with `pub struct Input { text, files }`
  (`loop_.rs`). No binding regenerated.
- **`est` drops the JSON envelope.** It now folds over parts (Text `len/4`,
  `Binary` → `IMAGE_TOKENS = 1600`, other → serialised `len/4`), so a plain
  text message estimates slightly lower than `to_string(msg).len()/4` did.
- **Vision hoisted as `Current::vision()`** (`loop_.rs`), passed to both
  `Frame {` sites; summary compaction runs `frame::strip_images` over its
  cloned history when `!vision`.
- **`check_files`** lives in `supervisor.rs` with `MAX_ATTACHMENT`; a rejected
  attachment publishes `AgentEvent::Error { session: <given or default>, turn:
  None }` and drops the input — the existing bad-session-id arm. Over ACP a
  rejected prompt therefore never gets a `TurnFinished`, same as a bad
  session id today.
- **ACP inbound**: `prompt_parts(blocks, data_dir) -> (String, Vec<PathBuf>)`
  plus private `save_image`; `connect(relay, data_dir)`. Extension from mime
  (`png jpg gif webp`); other image mimes become `[image: unsupported <mime>]`.
- **TUI**: `crate::clip` owns `Image`, `image()`, `downscale`, `encode_png`,
  `dropped_file`; `ui.rs` calls `clip::dropped_file` on every paste event and
  routes to `Model::attach` — `model.rs` stays I/O-free. The text-only notice
  in `attach` uses `catalog.closest(model)`, so it only fires when the
  catalog knows the model (never in the line printer). Image tokens are
  removed with one adjacent space at submit.
- **Deps**: `arboard 3.6` (`image-data`) pulls `image 0.25` and a second
  `png 0.18` transitively; our own encode stays on `png 0.17`. `mime_guess`
  added to `agent`; `sha2`/`base64` to `rpc`; `png`/`sha2` to `tui`.
