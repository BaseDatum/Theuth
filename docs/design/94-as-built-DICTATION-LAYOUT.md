# As-built — 94 DICTATION-LAYOUT — while dictating, the text fills the box on top and the voice bar spreads the full width underneath

Branch `feat/94-dictation-layout` in `~/Develop/theuth-wt-94`, based on local main `7d2f00ff`. Brief: `/tmp/mc-2026-10-04/omp-DICTATION-LAYOUT.md`.

Commits (oldest first):
- `0820f143` feat(desktop): dictation layout — text on top, full-width voice bar under it (94)
- docs(design): 94 as-built DICTATION-LAYOUT (this note)

## The ask

"Whenever I press the dictation button the little bubble that shows the vocal state actually splits the text input area in half … The bubble should instead spread the width of the text input area on the bottom, with all the text coming in on the top at full width."

## What changed — `crates/desktop/ui/src/components/chat/Composer.tsx` only

- `:1095-1118` while `dictating && dict.active` the text region (`:1092` `relative min-w-0 flex-1`) renders a `flex flex-col`: the unchanged `DictatedText` (`BOX` + the touch/desktop max-h) on top at full width, then the **voice bar** — a plain `<button type="button" aria-label="Stop dictation" onClick={dictate}>` with `mt-1 flex h-9 w-full items-center overflow-hidden rounded-full bg-fg-accent/10 px-1 text-fg-secondary hover:bg-fg-accent/15`, body `<VoiceWave state="listening" source={dictSource} label="Dictation level" className="h-full flex-1" />` and a lucide `<Square className="size-3.5 shrink-0 fill-current" />` at the right end as the stop glyph, wrapped in the same `Tooltip`/`TooltipTrigger asChild` with "Stop dictation (Ctrl+D)". Before `dict.phase === 'dictating'` `dictSource` is null and the wave shows its idle hum as before.
- `:1192-1206` the right-hand `h-9 w-32` wave pill is gone. The mic family is now `canDictate && !(dictating && dict.active) && <MicButton …/>` — while dictating it renders nothing. Talk/`+`/Send/Steer keep their rules (Send/Steer still disabled while dictating).
- `Square` was already imported from `lucide-react` (`:13`); no import changes. `Button` still has other users.

Untouched: `dictate()`, `Ctrl+D` (`theuth:dictate`), `dictation.stop/discard`, `pendingCaret`, `DictatedText`'s rendering, `RecordingBox`/voice-note pieces (89), `components/voice/VoiceWave.tsx`.

Row while dictating: [`+`] [text region = text on top, bar below] [Talk?] [Send/Steer].

## Gates

Bindings: `just ts-desktop` (log `/tmp/mc-2026-10-04/dict-ts.log`):

```
running 8 tests
........
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 2.88s

rm -rf crates/desktop/ui/src/rpc/bindings
mkdir -p crates/desktop/ui/src/rpc/bindings
cp -R bindings/. crates/desktop/ui/src/rpc/bindings/
```

(The trailing `EXIT=` line never landed in the log; the copy step completed — 464 files in `src/rpc/bindings` — and `check`/`test` resolve the bindings.)

`cd crates/desktop/ui && bun install && bun run check` (after `bun install` in `packages/apps`, needed for `@openuidev/*`):

```
$ tsc -b --noEmit
CHECK_EXIT=0
```

`CI=1 bun run test < /dev/null`:

```
 Test Files  131 passed (131)
      Tests  730 passed (730)
   Start at  10:36:56
   Duration  31.06s (import 58%, transform 40%, tests 2%)
```

`grep -rn "Stop dictation" src`: no test references the pill (only `Composer.tsx`, `SearchDialog.tsx`, `ui/dictation.tsx`, `rpc/api.ts` doc comment) — nothing to update.

## Deviations

- None from the decided design.
- Not visually smoked in the running app: the dictation path needs the Tauri backend with a loaded dictation model and a microphone. Proof is `tsc` + the full vitest suite; the JSX mirrors the brief's class list verbatim.
