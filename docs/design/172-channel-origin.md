# 172 CHANNEL-ORIGIN: show where a message came from in the desktop chat

Status: design (2026-11-03 17:05), two workers (ENG, UI); orchestrator integrates.

## Ask
Sky, 2026-11-03 17:00, by Slack DM, about his own Slack DM shown in the desktop chat:

> that's what I see in the UI when I send a slack DM.. that should be a custom UI element, and we need the same for signal and telegram. ideally with the correct icons to show where the message came from.

## Today (verified on main 7d5011a4)
- **Slack:** text inputs start with a marker line for the model, `[slack dm]` or `[slack channel]` (110 §7.1; `crates/slack/src/channel.rs`, around line 1400). Forwarded and linked messages follow as blocks from `crates/slack/src/forward.rs`: `[forwarded message from A · #chan · time · permalink]`, then `> body` lines, then `(channel …, ts …)`. Voice-note and file lines carry their own markers and no Slack marker. The desktop chat shows all of this raw in the user bubble.
- **Signal and Telegram:** they pass the owner's text as is, and an allowed user's as `name: text`. Nothing records the channel, so the desktop cannot tell a Signal or Telegram message from one typed on the desktop.
- **Existing provenance:** history rows already carry typed provenance. `HistoryEntry` user rows have `peer?: PeerOrigin` (design 50 §9, `crates/agent/src/communication.rs`) and `vision?: VisionNote`. Live user text reaches UIs on `AgentEvent::TurnStarted.text` and `AgentEvent::InputQueued.text`.
- **Icons:** the desktop UI has no brand icons.

## Design
- **D1, typed provenance, not text.** The channel that delivers a user row sets a new `ChannelOrigin` on it. The origin is stored with the row, replayed in history and carried live. The model's input is unchanged:
  - Slack keeps its marker line, and Signal and Telegram get none;
  - no prompt change, and no change to 110 §7.1.
- **D2, the type.** It lives in the agent crate and is exported with ts-rs:
  ```rust
  /// 172: which channel delivered a user row (UIs show it as a chip; the model input is unchanged).
  pub struct ChannelOrigin {
      /// slack | signal | telegram
      pub via: ChannelVia,
      /// dm | group — a Slack channel and a Telegram group, supergroup or forum topic are `group`; Signal is DM-only today.
      pub chat: ChannelChat,
      /// The sender's label when it is not the owner (an allowed user), as the channel already names them.
      pub from: Option<String>,   // absent when None
      /// The channel or group title when the channel knows it (Slack channel name without `#`, Telegram group title).
      pub title: Option<String>,  // absent when None
  }
  pub enum ChannelVia { Slack, Signal, Telegram }  // serde + ts lowercase
  pub enum ChannelChat { Dm, Group }               // serde + ts lowercase
  ```
  The generated TypeScript must be exactly:
  - `ChannelVia.ts`: `export type ChannelVia = "slack" | "signal" | "telegram";`
  - `ChannelChat.ts`: `export type ChannelChat = "dm" | "group";`
  - `ChannelOrigin.ts`: `export type ChannelOrigin = { via: ChannelVia, chat: ChannelChat, from?: string, title?: string, };`, with doc comments allowed.
- **D3, where it rides.**
  - `SendInput.channel` and `Steer.channel` (`Option<ChannelOrigin>`, serde default `None`).
  - The stored user row, beside `peer`. Rows written before 172 read back as `None`.
  - `HistoryEntry` user `channel?: ChannelOrigin`.
  - Live: `AgentEvent::TurnStarted.channel?: ChannelOrigin` and `AgentEvent::InputQueued.channel?: ChannelOrigin`.
  - Every input path of the three channels sets it: text, voice notes, files and images, forwarded or linked messages, steers, new root sessions, bound threads and child sessions.
  - Everything else (desktop, TUI, schedules, peers, the RPC `send_input` from UIs) sends `None`.
- **D4, the chip.** A user row with `channel` shows a small origin chip on its bubble, made of the channel icon and a label:
  - "Slack · DM", or "Slack · #general" when `title` is set, else "Slack · channel";
  - "Signal";
  - "Telegram · DM", or "Telegram · \<title\>", else "Telegram · group";
  - followed by " · from \<from\>" when `from` is set.

  The bubble text drops one leading `[slack dm]` or `[slack channel]` line. Rows stored before 172 have no `channel`, so when their text starts with that marker line the UI derives the same chip from it (Slack, `dm` or `group`, no title). Today's history renders the same way.
- **D5, forwarded and linked Slack messages.** Each block becomes a quote card instead of raw lines:
  - a header with "Forwarded message" or "Linked message", the author, `#chan` and the time, with the permalink as a link;
  - the quoted body;
  - the `(channel …, ts …)` line hidden from view.

  An `[attachment from …]` line stays one muted line. Anything the grammar does not admit stays as text.
- **D6, icons.** Slack, Signal and Telegram glyphs as inline SVG components, with no new dependency. Take the paths from Simple Icons (CC0-1.0) and credit them in a comment. They must be recognisable at 14 px in light and dark themes. Brand colour or `currentColor` is the UI worker's call; document it.
- **D7, out of scope.** The TUI, phone push notifications, and sidebar icons for channel-bound sessions are open items.

## Contract between the workers
- ENG produces exactly the TypeScript in D2 and D3 through the bindings export.
- UI codes against a hand-written stub of the same bindings. The bindings directory is gitignored, so the stub is never committed.
- The orchestrator regenerates the bindings at integration and runs tsc.
