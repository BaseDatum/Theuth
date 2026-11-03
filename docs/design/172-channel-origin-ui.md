# 172 CHANNEL-ORIGIN UI: as built (D4–D6)

Status: as built 2026-11-03, branch `feat/172-channel-origin-ui`, against the D2/D3 TypeScript contract in
`172-channel-origin.md` (stubbed bindings; the orchestrator regenerates them at integration).

## What was built, where (all under `crates/desktop/ui/src`)
- **Data (D3 consumer).**
  - `store/sessions.ts`: the `user` entry gets `channel?: ChannelOrigin`; `queued` items get `channel?` too.
  - `store/reduce.ts`: `userEntries(…, channel)` puts `channel` on the `you` row (plain and skill rows). Its sources:
    - history: `HistoryEntry` user `channel`;
    - live: `TurnStarted.channel`;
    - `InputQueued.channel`, kept on the queued item and moved onto the transcript row on `InputInjected`. The `TurnStarted` echo of an injected steer is skipped as before, so the origin survives.
- **`lib/channel-origin.ts`** (pure):
  - `originOf(entry)`: the typed `channel`, else, for rows stored before 172, a leading `[slack dm]` line gives `{via:'slack', chat:'dm'}` and `[slack channel]` gives `chat:'group'`.
  - `stripMarker(text)`: drops exactly one leading marker line.
  - `originLabel(origin)`: the D4 labels.
  - `parseShared(text)`: the grammar below.
  - `previewText(text)` / `stripSnippet(text, ranges)`: preview helpers for the sidebar, header and search.
- **Rendering (D4/D5).** `components/chat/MessageUser.tsx` (passed `channel` by `Message.tsx`):
  - `OriginChip` is the tag (`rounded-full bg-layer-2 px-2 text-xs leading-5 text-fg-secondary`, the same tag style `BoardQuoteCard` uses) with a 14 px `ChannelIcon` and the label. It sits at the top of the message group and is truncated to the bubble's max width.
  - The marker line is stripped before the existing pipeline (attach refs, voice note, spoken, board ask, screen lines).
  - For a Slack origin only, the remaining words go through `parseShared`:
    - text segments are ordinary bubbles;
    - forwarded and linked blocks are a `SharedCard`, styled like `BoardQuoteCard` (`rounded-xl border bg-layer-1`). Its header has the forward or link icon, "Forwarded message" or "Linked message", the author (emphasised), `#chan`, the time, and "Open in Slack" (`openExternal`; it stops propagation so the bubble's stamp toggle does not fire). Then the body with a left accent rule, then `files: …` muted;
    - an attachment is one muted, truncated line (paperclip, `from · text`, with the full text in `title`).
  - A desktop-typed message has no `channel` and no marker, so it gets no chip.
- **Previews.**
  - `sidebar/SessionItem.tsx` and `chat/Header.tsx` titles go through `previewText`.
  - `sidebar/SearchDialog.tsx` `SessionRow`: the hit title goes through `previewText`, and the title and message snippets go through `SessionSnippet` (`stripSnippet` and `Snippet`, with ranges shifted).
  - Notifications show no user text (`lib/notify.ts` is only called by `AlertNotifier`), so nothing changed there.
- **Icons (D6).** `components/icons/channels.tsx`: `SlackIcon`, `SignalIcon`, `TelegramIcon` and `ChannelIcon({via})`, inline SVG with no dependency.
- **`openExternal` moved** from `components/attention/Pointers.tsx` to `lib/platform.ts`, with the same body and its callers repointed (`Pointers`, `PacketMarkdown`, `settings/Channels`, `MessageUser`). Reason: importing it from `Pointers` pulled the artifact `Viewer` and its router API into the chat bubble's module graph, and the existing `ScreenContextCard.test.ts` (which renders `MessageUser` under a two-export router mock) failed on it. `lib/platform` is where "how this container opens things" already lives.

## Icon source, licence, colour
- **Paths.** From Simple Icons (CC0-1.0), credited in the file header, verified byte-for-byte against the jsDelivr copies:
  - `signal` and `telegram` from 16.36.0;
  - `slack` from 15.22.0. Simple Icons 16.0.0 (2025-11-30) removed Slack together with the other Salesforce marks, at the brand owner's request. The 15.x path is still CC0 as published, but the trademark is Slack's (see Open items).
- **Brand colours, not `currentColor`.** At 14 px the colour carries most of the recognition: Slack's four-colour hash, Signal blue, Telegram blue. A monochrome glyph in muted text reads as a generic blob.
  - Slack's single Simple Icons path is split into its eight subpaths, two per lobe, coloured as Slack's mark: `#E01E5A` bottom-left, `#36C5F0` top-left, `#2EB67D` top-right, `#ECB22E` bottom-right.
  - Signal uses `#3B45FD` (Simple Icons' hex) and Telegram `#26A5E4`.
  - All of them read on both the light (bone) and dark tag backgrounds; see `icons-1x-*.png`.
  - At 1× the Signal mark's dashed ring merges into a blue speech bubble. It is still recognisable by shape and colour, and it is the brand's own mark.

## Parser grammar (`parseShared`, from `crates/slack/src/forward.rs` `render`/`shared`/`other`)
```
body       := line*   (each line: a shared block, an attachment, or plain text)
shared     := header quote* files? source?
header     := "[" ("forwarded"|"linked") " message from " author (" · #" chan)? (" · " time)? (" · " url)? "]"
              time = YYYY-MM-DD HH:MM UTC; url = http(s)://…; optional fields at most once, in this order
quote      := ">" | "> " text                     (the "> " is removed; ">" alone is an empty line)
files      := "(files: " names ")"                (shown muted)
source     := "(" ("channel"|"ts"|"thread_ts") " " id (", " ("ts"|"thread_ts") " " id)* ")"   (dropped)
attachment := "[attachment" (" from " label)? "]" (" " text)?
```
- A header with zero quote lines is admitted: `shared` writes no `>` line for an empty body.
- A malformed source line after a block stays text.
- Anything else stays text: a header with unknown or out-of-order fields, a bare quote, a bare `(channel …)` line.
- The parser runs only on Slack-origin rows, so a desktop message that happens to contain `[attachment]` is untouched.

## Proof (`/tmp/172/proof-ui/`)
- **Vitest.**
  - `src/lib/channel-origin.test.ts`:
    - `originOf`, including no chip for a desktop message, `[voice note, N s] …`, and text that merely starts like the marker (`[slack dm] hi`, `[slack dms]`, `[slack]`, a leading space, a marker that is not on the first line);
    - `stripMarker`, `originLabel` (every D4 form);
    - `parseShared` on the slack crate's own fixtures (`channel.rs` around 3720–3780), the linked, empty-quote and files cases, and off-grammar input;
    - the preview and snippet helpers.
  - `src/store/reduce.test.ts`, "a user row keeps its channel origin…": history, `TurnStarted`, and a queued steer that is injected and then echoed, plus desktop rows without one.
  - Output: `vitest.txt`.
- **Screenshots.** `{chromium,webkit}-{light,dark}-{desktop,phone}.png`:
  - headless Playwright 1.64.1 (`/tmp/172/pw`, not a repo dependency). WebKit 2370 is the matching build; Chromium is the installed headless shell 1226, through `executablePath`;
  - desktop is 1280×800 and phone 390×844, at 2× scale, full page;
  - `icons-1x-{light,dark}.png` shows the chips at 1×, i.e. the true 14 px icon size.
  - The fixtures are the eight from the brief. They are rendered through the real `fromHistory` and the transcript `Message` component on a throwaway harness page served by this worktree's vite on port 1472. The page was deleted after the shots; a copy is in `harness/` along with the shoot scripts.
  - `shoot.log` records, per shot, the chips found (7, one per channel fixture; none for the desktop row), one shared card, `rawMarkerVisible: false`, and no page errors.
- **Contract stub.** `ui-stub.patch` (also at `/tmp/172/ui-stub.patch`) is `diff -ruN /tmp/172/bindings-main` against the stubbed `src/rpc/bindings`.
- **tsc.** `tsc.txt`: no errors beyond the baseline.

## Decisions and deviations
- **Edit uses the stripped text.** Editing a Slack-origin turn from the desktop (Pencil) starts from the text without the marker line, because the edit is a desktop send. A `[slack dm]` line there would make the model write for Slack while the reply lands on the desktop.
- **Search snippets.** A snippet is stripped only when it starts at the row's first character with the marker. A snippet cut mid-row starts with `…` and is left alone. A snippet that matched only inside the marker shows the chip label, without highlight.
- **The time in the card is shown as Slack rendered it** (`2026-10-30 14:02 UTC`), not converted to local time. The header text is all the UI has, and it is unambiguous.
- **The "from" prefix is not stripped.** Signal and Telegram allowed users, and Slack, prefix the body with `name: `, and the chip also says "· from name". The duplication is kept rather than editing the body on a guess (see Open items).
- **`openExternal` moved** to `lib/platform.ts`; reason above. This is a deviation from "use the helper in `Pointers.tsx`": it is the same helper in a lighter module.

## Open items
- **Title seeding (engine, D1–D3 side).** `attach::first_line` seeds a Slack-started session's title as the literal `[slack dm]`. The UI now shows the chip label ("Slack · DM") for such a title, but the engine should skip the marker line when seeding.
- **Queued steers after a reload.** `list_queued` (`QueuedInput`) carries no `channel`, so a steer queued before a reload loses its origin. The fix is to add `channel` to `QueuedInput` if it matters.
- **The duplicated `name: ` prefix** for allowed users: strip it in the bubble when `channel.from` matches, if Sky wants that.
- **Slack trademark.** The 15.x Slack path is CC0, but Slack/Salesforce asked Simple Icons to remove it. Showing Slack's logo to indicate the Slack integration is ordinary nominative use; check Slack's brand guidelines before shipping it more widely.
- **D7 out of scope:** the TUI, push notifications, sidebar icons for channel-bound sessions, and a chip on the queued-steer chips (`QueuedChips`).
