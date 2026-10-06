# 108 — as built, UI worker

Branch `feat/108-ui` on `8490bb5a`. Four commits (`bd7b1c4b`, `5125f9ab`, `ed20b3e8` + this note). Everything in §11's UI column
landed, including §6.6 Edit by chat.

## Files

| file | change |
|---|---|
| `components/routines/helpers.ts` | `cronWords` gains *at* before clock times; `onceAt` locale-free (`7 Oct 15:00`); `Readable`, `fromInfo`, `fromFile`, `sentence(r, args?)` (§2.2), `kindOf` (D6), `parseWhen`; `resultKind(r, kind?)` with the fourth `check` kind + `resultLabel`; `plainError` (§5.5 map) |
| `components/routines/helpers.test.ts` | every §2.2 example as a `test.each` fixture; header-filtered subject; gist/partial rules (c)(d); `fromInfo` ≡ `fromFile`; `kindOf`; check kinds + `RunInfo.kind`; §5.5 map; `draftToFile` round trip incl. `latestDraft`/`fileText`; §6.2 five-call order against a mocked `api` |
| `components/routines/draft.ts` | `RoutineDraft`/`RoutineDraftArgs` (§8.1 shape), `NEW`, `draftToFile`, `latestDraft(entries)`, `fileText`, `editSeed`, `startRoutineChat` (§6.2) |
| `components/routines/RoutinesView.tsx` | §3 row: `KindBadge` · name · state · `StopChip`; sentence line (title = raw clauses); health line; Describe it / New routine header; §3 empty state; mobile stacking, Test hidden below `md` |
| `components/routines/RoutineDetail.tsx` | §4 sentence + four `Fact` rows, `VerdictCard`, History with `RunInfo.kind` (folded checks, envelope chips, `end` lines); §5 form: Trigger / Stop / Run / Concurrency fieldsets with Advanced disclosures, sensor picker + `# arg:` inputs + `list_secrets` datalist, fire/mode radios, live sentence, Test now, Create paused / Create and start, §5.5 errors under fields; §6.3 pane layout (Divider ≥ md, bottom Sheet on phones), §6.4 draft→form, §6.5 presses, §6.6 Edit by chat |
| `components/routines/RoutineChat.tsx` | the pane (intro + `LiveSession first={false}`), `RoutineDraftBridge` |
| `components/routines/queries.ts` | `useRoutinesApi` (fixture-aware), `useSensors` |
| `routes/routines.new.tsx`, `routes/routines.$name.$section.tsx` | `validateSearch` for `?chat=` |
| `routes/__root.tsx` | mounts `RoutineDraftBridge` |
| `components/apps/AppHost.tsx` | one `dispatchEvent('theuth:app-press')` before the send (D9) |
| `components/chat/MessageTool.tsx` | `|| tool.name === 'routine_draft'` |
| `rpc/api.ts` | `test_schedule_file`, `list_sensors` + METHODS |
| `rpc/bindings/{SensorArg,SensorInfo,RunKind}.ts`, `RunInfo.ts`, `ScheduleInfo.ts`, `methods.json`; root `bindings/methods.json` | hand-written §9.6 stubs (gitignored) |
| `dev/routines-fixture.ts` | `VITE_ROUTINES_FIXTURE=1`: the routine RPCs answered from memory |

## Judgement calls beyond the doc

- **Subject hides secrets.** §2.2 says "required args only when the header is known", yet every example drops `token=…` (a required arg). Rule as built: args named `token`, or whose header example is `secret`, or optional-by-header, are left out of the subject. Pinned by tests.
- **`missed:` is not in `ScheduleInfo`** (§9.3 does not add it), so `fromInfo` sets `missed_run: false` and the list sentence cannot say "catching up a missed run"; the detail form and the live preview can.
- **`Divider dir="row"`**, not `"col"`: `Panes.tsx:71` names the *separator* direction — `col` is the horizontal bar ControlCenter uses to stack chat *under* the transcript; two side-by-side columns need `row`. File wins over doc.
- **`?chat=1` arrives as the number 1** through TanStack's default search parser; `chatSearch` accepts the number. Links write `chat=%221%22` (string round-trip); both open the intro.
- **`RoutineChat` has no `onDraft`/`onOpenForm` props**: the page owns the slice subscription (`useSessions` → `latestDraft`) and the press listener; the pane only needs `session/onSession/seed/onClose`.
- **Edit-by-chat seed is derived**, not state: on a detail page with `?chat=1` the seed is `{ name, text: fileText(file) }`, so a reload keeps the edit framing. `fileText` renders the keys in §2.1 order with defaults omitted; it is context for the model, not a byte-exact copy of the engine's render.
- **Form create keeps the pane**: `Create paused`/`Create and start` from `/routines/new?chat=<id>` navigate to the new Overview with `?chat=<id>`, same as a card press.
- **"Replace your edits with the new draft?"** is `window.confirm` (native, rung 4). A declined draft is remembered so the question is asked once per card revision.
- **Run fact row** shows `session · last · run keys`; the `RoutineChips` there omit the last-result chip (it is already in the text) — the list row keeps it.
- **`enabled` switch left out of the form** on the detail page: the header Switch (`enable_schedule`) is the one control; Save writes the file's current `enabled`. On `/routines/new` the two Create buttons set it.
- **Fixture is an `Api` overlay**, not a connection: the app needs an engine to boot (`App.tsx` gating); with `VITE_ROUTINES_FIXTURE=1` the routine methods are answered in memory against any daemon, including one without `list_sensors`/`test_schedule_file`/`RunInfo.kind`.
- `ToolRunCard.tsx` on this base has no `routine_draft` exemption; per the brief, not added.

## Gates (in `crates/desktop/ui`)

```
$ bun run check
$ tsc -b --noEmit
check exit=0                      # 0 errors total; 0 in owned files
$ CI=1 bun run test
 Test Files  1 failed | 141 passed (142)
      Tests  1 failed | 823 passed (824)   # api.test.ts: root bindings/methods.json lacked the +2 names
$ CI=1 bunx vitest run src/rpc/api.test.ts   # after adding list_sensors / test_schedule_file to root bindings/methods.json
      Tests  2 passed (2)
$ CI=1 bunx vitest run src/components/routines
      Tests  24 passed (24)
$ bun run build
✓ built in 20.77s
```

## Smoke (vite dev + headless Chromium via the harness `browser`)

Throwaway jsonrpsee-shaped WS daemon (`info`, subscriptions, lists → defaults; `use_skill` pushed one `ToolCallStarted routine_draft`) with `VITE_THEUTH_URL/TOKEN` and `VITE_ROUTINES_FIXTURE=1`; deleted afterwards. Observed at 1280×800 and 390×844:

- `/routines`: badges Clock/Once/Watcher/Loop, stop chips (`runs 3/40 · expires 14d`), sentences, `quiet`/`unchanged` as grey chips (no red), broken row red; mobile stacks lines and controls, Test hidden.
- `/routines/pr-482/overview`: sentence with gist; Trigger/Stop/Run/Concurrency rows (`expires 14d (11d 23h left)`); form sections with sensor + args filled from `when:`.
- `/routines/pr-482/history`: `3 checks · quiet ×2, unchanged ×1 · 11:50–11:52` folds, envelope chips `subject/stage/events`, amber `skipped`, red `failed` only on a run.
- `/routines/new?chat=1`: pane intro + three chips; "Describe it" → URL `?chat=fake-…`, `LiveSession` mounted; the pushed `routine_draft` filled name, trigger, sensor, args, stop, run keys and the sentence line with "from the chat draft · 11:55 AM"; `Create paused` landed on the new Overview.
- Detail `Edit by chat` → `?chat=1` with "What should change about pr-482…"; `Test now` rendered the verdict card ("would not fire now"); clearing a watcher's stop and saving showed "A watcher needs an end: … (when: needs a stop (until, max_runs or expires))" under the Stop section.
- Phone: the pane is a full-height bottom Sheet with ✕ Close chat.

## Not exercised

- A real `routine_draft` card press through `AppHost` (the OpenUI card needs the engine's `routine_draft` tool); the `theuth:app-press` path was driven by dispatching the event by hand — navigation on `create paused` with `?chat` preserved, and `open in form` scroll/sheet-close were exercised only by code reading.
- `RoutineDraftBridge` from `/c/<id>` (no card in an ordinary chat without the engine).
- `test_schedule_file`, `list_sensors`, `RunInfo.kind` against the real engine (fixture only); the stubs match §9.6 and must be replaced by `just ts-desktop` at integration.
- `list_secrets` datalist contents against a real vault.
- `pairing.test.ts` / `mobile-zoom` did not flake in this run; not rerun alone.
