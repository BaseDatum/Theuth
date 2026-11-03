# 164 CHANNEL-PICTURES — diagrams and surfaces arrive as pictures on Signal, Telegram and Slack

Status: contract (orchestrator, 2026-11-03). Workers append as-built notes in `164-as-built-ENG.md` / `164-as-built-UI.md`.

## Ask (Sky, 2026-11-03, verbatim)

> when the user is communicating with the agent over signal, telegram, or slack, we need to actually handle the json render and other types of diagrams, render them and attach them to the message sent over the channel to the user and omit the raw json from the messages.  everything should still be handled the same in the mobile and desktop app, but we need to not just send the user a bunch of raw json on signal etc and instead render it and attach those as images to the message sent over the channel, while leaving the display and treatment exactly the same in the main UI.

## Mechanism today (verified on main cc757054)

- Signal `crates/signal/src/channel.rs:1811 flush()` sends `markdown_to_plain(surface::flatten(&md, 60))` (:1834). `surface::flatten` (`crates/surface/src/flatten.rs`) only replaces a closed `json-render` fence that parses AND fits 60 columns; every other fence — wide diagrams, `mermaid`, `openui`, broken JSON — goes out byte-identical, i.e. raw JSON/source.
- Telegram never calls `surface` at all (the crate does not even depend on it): every fence goes out raw in drafts (`bubble.rs flush` :209) and in the persisted rich message (`close` :290).
- Slack (`crates/slack/src/bubble.rs` :161–:250) posts the design-110 Block Kit subset natively and flattens the rest at 60 columns — the same raw-JSON leak for wide diagrams, mermaid and openui.
- `message_user` deliveries (`crates/{signal,telegram,slack}/src/user.rs`) go out the same way.
- Precedent for client rendering: `page_preview` (`crates/artifact/src/preview.rs`) publishes `PagePreviewRequested`, a connected desktop/web client renders hidden and answers through RPC `answer_page_preview`; `doc_export` embeds client-rendered diagram SVGs (design 72).

## Decisions (D1–D12; decided — do not re-litigate)

- **D1 Scope.** Only what the Signal, Telegram and Slack adapters SEND changes. The stored transcript, the desktop/web/phone UI, the TUI and every other surface stay byte-for-byte as today.
- **D2 Picture fences.** Closed fences whose info string is `json-render` (alias `jsonrender`), `mermaid` or `openui`. Every such fence in an outbound reply, `message_user` text or schedule delivery is either sent as a PNG or replaced by a text fallback (D7). **A raw fence body never reaches a channel** — open or closed, broken or valid, streaming or final.
- **D3 Rendering = connected desktop/web client, page_preview's shape.** The engine publishes `FenceSnapshotRequested`; the main window of every connected desktop or web client (never the phone, never AUX windows) answers `started`, renders the fence off-screen with the SAME component the chat uses for that language, rasterizes it to PNG and answers `done` (or `failed`) through RPC `answer_fence_snapshot`. `FenceSnapshotResolved` tears late renders down. First terminal answer wins. No `started` within 3 s → no renderer; no terminal answer within 20 s of the request → timed out. A `NoRenderer` outcome makes the engine skip new requests for 60 s (straight to D7), reset by any later answer.
- **D4 Request parameters.** `appearance: light`; `scale: 2`; `width` 560 CSS px for Signal and Telegram (phones), 720 for Slack. The PNG background is opaque (the light page background). The client keeps the PNG ≤ 4 MB and each side ≤ 8000 px by lowering scale (2 → 1.5 → 1); still too big → `failed`.
- **D5 Cache.** The engine keeps the last 64 `done` PNGs keyed by sha256(lang, source, width, appearance, scale); retries, Slack updates and Telegram repairs reuse them.
- **D6 Order and captions.** Pictures keep their place in the reply: text before the fence, then the picture, then the text after. The fence is removed from the prose (no inline marker when the picture follows). Caption/title = the root json-render element's `title` prop when it has one; otherwise none (Slack file title `diagram`). Slack `alt_txt` = the first line of the fence's text lowering.
- **D7 Fallback (no renderer, failed, timed out, `pictures: false`).** Never the raw body:
  - json-render that parses → today's 60-col text box when `spec_lines_text` fits, else the compact markdown of `surface::export::lower`, else a one-line placeholder;
  - mermaid → `surface::mermaid_lines(body, 60)` box when it renders, else placeholder;
  - openui → placeholder; json-render that does not parse → placeholder;
  - placeholder text: `[<title or caption or "diagram"/"interactive panel"> — open theuth to see it]`.
- **D8 Live previews.** A Telegram draft/edit and a Slack stream never show a fence body: while a picture fence is open, or closed but not yet posted, the live text shows the prose before it plus one italic line `_drawing…_`.
- **D9 Per channel.**
  - Signal: per `flush()` block, request all its fences concurrently, then send text runs and pictures in order (`send_attachment(to, png, "image/png", Some("<name>.png"), caption)`). The TTS clip of a voice turn never reads a fence body.
  - Telegram: a closed picture fence is a block boundary — finish the bubble before it, send the picture with `send_file` (photo; document when Telegram's photo limits are exceeded: > 10 MB, w + h > 10000 or a side ratio > 20), continue in a new bubble.
  - Slack: fences `surface::slack_blocks` renders within the block cap keep going out as Block Kit (design 110 ruling); every other picture fence (json-render without a block form or over the cap, mermaid, openui) becomes a PNG uploaded into the thread at its position with `upload_file` (files v2), in stream and update modes. With `slack.blocks: false` every json-render fence is a picture.
- **D10 Config.** `signal.pictures`, `telegram.pictures`, `slack.pictures` (bool, default `true`); `false` = D7 only. No Settings UI change.
- **D11 Slack prompt.** `crates/slack/src/prompt.rs`: the "Draw nothing Slack cannot show: no mermaid, no openui, and no json-render except …" sentence becomes: diagrams (json-render, mermaid) arrive as pictures, Alert/Callout/Table/Bars/Sparkline/Metric/MetricGrid/KeyValueList/Checklist as native blocks; no openui. Signal and Telegram have no prompt section; none is added.
- **D12 Platforms.** Engine code is platform-independent; the client runner works in the desktop shell on macOS, Windows and Linux and in a plain browser.

## Contract

### Rust (`crates/artifact/src/snapshot.rs`, every type `#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]`)

```rust
/// Correlates a snapshot request with its answers.
#[serde(transparent)]
pub struct SnapshotId(pub uuid::Uuid);

/// The fence languages a client draws.
#[serde(rename_all = "kebab-case")]
pub enum FenceLang { JsonRender, Mermaid, Openui }

/// Event: a channel reply wants a fence drawn as a PNG.
#[serde(tag = "type")]
pub struct FenceSnapshotRequested {
    pub id: SnapshotId,
    pub lang: FenceLang,
    /// The fence body verbatim (between the fence lines).
    pub source: String,
    /// Frame width in CSS px.
    pub width: u32,
    /// Palette to draw under (`crate::preview::Appearance`).
    pub appearance: Appearance,
    /// Device-pixel ratio of the PNG (1–3).
    pub scale: u32,
}

/// Event: the request was answered or timed out; clients drop late renders.
#[serde(tag = "type")]
pub struct FenceSnapshotResolved { pub id: SnapshotId }

/// A client's answer (`crate::preview::Client`).
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FenceSnapshotAnswer {
    Started { client: Client },
    /// `png`: standard base64 of the PNG bytes, no `data:` prefix; `width`/`height` in PNG pixels.
    Done { png: String, width: u32, height: u32, client: Client },
    Failed { reason: String, client: Client },
}
```

RPC (mirrors `answer_page_preview`): `answer_fence_snapshot(id: SnapshotId, answer: FenceSnapshotAnswer) -> bool` — `true` while the request was pending.

### TypeScript exactly as ts-rs emits it (`crates/desktop/ui/src/rpc/bindings/`, gitignored)

```ts
// SnapshotId.ts
export type SnapshotId = string;
// FenceLang.ts
export type FenceLang = "json-render" | "mermaid" | "openui";
// FenceSnapshotRequested.ts
import type { Appearance } from "./Appearance";
import type { FenceLang } from "./FenceLang";
import type { SnapshotId } from "./SnapshotId";
export type FenceSnapshotRequested = { "type": "FenceSnapshotRequested", id: SnapshotId, lang: FenceLang, source: string, width: number, appearance: Appearance, scale: number, };
// FenceSnapshotResolved.ts
import type { SnapshotId } from "./SnapshotId";
export type FenceSnapshotResolved = { "type": "FenceSnapshotResolved", id: SnapshotId, };
// FenceSnapshotAnswer.ts
import type { Client } from "./Client";
export type FenceSnapshotAnswer = { "kind": "started", client: Client, } | { "kind": "done", png: string, width: number, height: number, client: Client, } | { "kind": "failed", reason: string, client: Client, };
```

`src/rpc/api.ts`: `answer_fence_snapshot(id: SnapshotId, answer: FenceSnapshotAnswer): Promise<boolean>` plus `'answer_fence_snapshot'` in the METHODS list; `bindings/methods.json` gains the same name.
