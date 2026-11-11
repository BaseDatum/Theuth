# 192 COMPANION-PICTURES — engine contract

The native companions (macOS Swift, Windows C#) show `json-render`, `mermaid` and `openui`
fences as pictures drawn by the main app's design-164 renderer, through one engine RPC.

## Method

`fence_snapshot`, positional params `[lang, source, width, appearance, scale]`:

- `lang`: `FenceLang` — `"json-render"` | `"mermaid"` | `"openui"`;
- `source`: String, the fence body without the ` ``` ` lines;
- `width`: u32 CSS px, clamped to 240..=1200;
- `appearance`: `Appearance` — `"dark"` | `"light"`;
- `scale`: u32 device pixel ratio, clamped to 1..=3.

## Result

`FenceSnapshotReply`, JSON exactly:

```json
{"outcome": "done"|"no_renderer"|"failed"|"timed_out"|"cancelled",
 "png": <standard base64, no data: prefix>|null,
 "width": <PNG px>|null,
 "height": <PNG px>|null,
 "error": <string>|null}
```

`png`, `width` and `height` are non-null iff `outcome` is `"done"`. `error` is the message for
`"failed"`, null otherwise. Snake_case field names, outcome values snake_case.

## Auth

The same authenticated-peer requirement as other methods (no peer → `UNAUTHORIZED`). Guest
peers (mode-2 document guests, `peer.guest()` is `Some`) are refused with `UNAUTHORIZED`
(`owner only`) before any request goes out. The call is read-only.

## Behaviour

The engine calls `artifact::snapshot::render` with the clamped values (164: 64-entry cache,
3 s with no `Started` → `no_renderer` and a 60 s back-off, 20 s cap) and maps the outcome.
