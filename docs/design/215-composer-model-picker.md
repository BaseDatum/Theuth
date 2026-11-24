# 215 — The model picker lives in the chat input

Owner, 2026-11-24. Supersedes the 2026-11-24 rule "the new-chat page owns the picker; live sessions keep the header menu".

## Goals
G1. Every chat input (every `<Composer>` host: chat route, talk route, new chat, notebook ask view, notebook chat column, note chat) shows the model picker **inside the input**, as a quiet muted label on the toolbar row ("Option B" of the mockup).
G2. Every other model picker in chat chrome is removed: Header `model` item (ModelMenu, ModelRow, ModelDialog), NewChatPane's row above the composer, ChatColumn's and NoteChat's header ModelMenu, the dev fixture row.
G3. Every non-chat model field (`ModelCombobox`, ~21 sites in settings, agents, routines, meetings, autoresearch, MemoryPanel) uses the same quiet-label trigger style instead of the outline button. Behaviour (popover/dialog, ModelPicker body, effort chips, none row, favorites) is unchanged.
G4. The input box becomes square-ish, not a pill: the **entire top line(s) are the text**, and **all existing buttons** sit on one toolbar row below the text.
G5. Skill pills, the prompt-suffix pill (and any sibling pill above the box) flow **horizontally, left-aligned**, wrapping only when out of room.

## C1. `ComposerModel` (owned by the pickers builder, file `components/chat/ModelMenu.tsx`)
```ts
export function ComposerModel(props: { session: string | null; own?: boolean; slash?: boolean }): JSX.Element
```
- Trigger: ghost, no border, `h-8 max-w-[40%] min-w-0 gap-1 rounded-md px-2 text-[13px] font-normal text-fg-tertiary hover:bg-layer-2 hover:text-fg-secondary data-[state=open]:bg-layer-2 data-[state=open]:text-fg-secondary`; content `name` (truncate) then `· effort` (shrink-0), `LocalTag` if local, `ChevronDown size-3.5 opacity-60`. Name truncates first.
- Desktop: `PopoverContent side="top" align="end" sideOffset={8} className="w-80 p-0"`. Touch: the existing top-of-band Dialog.
- Carries every `ModelMenu` behaviour: session/draft model (`ModelCommand` + Advisor + Knowledge harvester rows), ACP `HarnessPicker`, pending "after this turn", watching → plain muted text span, harness gone → muted text + Resume, `useOpenOnSlash` (`/model`, `/effort`, SessionMenu "Model: …" row) and `useOpenAdvisor` (AdvisorPill "Change model…").
- Wrapped in `<Capability required={modelRequirements} hide>` by `ComposerModel` itself, so hosts need nothing.
- `ModelMenu`, `ModelRow`, `ModelDialog` may be deleted once unused (keep exported helpers others import: `useModels`, `useFavorites`, `dialogPlacement`, `openAdvisorPicker`, `harnessGroups`, `harnessCurrent`, `HarnessPicker`, `AdvisorCommand`, `KnowledgeHarvesterCommand`, `slashTarget`, `type Model`). Tests in `ModelMenu.test.ts` that mount `ModelMenu` move to `ComposerModel`.
- The contract commit ships a stub body delegating to `ModelMenu small`; the composer builder codes against the signature only.

## C2. Composer (owned by the composer builder, file `components/chat/Composer.tsx` + pill files)
- New prop `ownModel?: boolean` → forwarded as `own` to `ComposerModel`.
- Box: keep the same element with `ref={box}` and `data-composer=""`. Radius `rounded-2xl` (not 28px), padding about `px-3 pt-2.5 pb-2`. Row 1: the text region (`RecordingBox` / `DictatedText` / `Textarea`) full width. Row 2 toolbar `flex items-center gap-1`: left = `DeleteNote`/`RecordingMeta` (voice-hold), `+` Add popover, `McpMenu`; `flex-1` spacer; right = `<ComposerModel session={session} own={ownModel} />` (hidden when `resolveSession` is set), Talk group / Talk, `MicButton`, Steer+Stop or Send. Every existing button, condition and aria-label is preserved; nothing is removed.
- Icon buttons may shrink to `size-8`; round icon buttons are fine. The dictation stop bar and talk group lose `rounded-full` in favour of `rounded-lg`.
- `useNarrow` should measure the toolbar/box (the text is now full width).
- Pills (G5): one `flex flex-wrap items-center justify-start gap-1.5 pb-2` row holds the skill chips and the suffix pill; `SkillChips` renders its chips without its own full-width wrapper (fragment or `bare` prop); each pill `max-w-[240px] min-w-0`, names `truncate`. Keep `data-skill-chip` / `data-suffix` attributes. QueuedChips stays its own row.
- Source-order pins in `Composer.test.ts` (`<QueuedChips` < `{strip}` < `ref={box}`), `NextSteps.test.ts` and `send-parked.test.ts` string pins must stay green.

## C3. Hosts and chrome (owned by the pickers builder)
- `Header.tsx`: remove `render.model`, `ModelRow`, `ModelDialog`, `picker` state, imports; `lib/header.ts` drop `HeaderId 'model'`, its `headerOrder` entry and `HEADER_REQUIREMENTS.model`; update `lib/header.test.ts`.
- `NewChatPane.tsx`: remove the ModelMenu row above the composer.
- `ChatColumn.tsx`: remove the header `ModelMenu`, pass `ownModel` to its `<Composer>`. `NoteChat.tsx`: remove the header picker.
- `dev/effort-fixture.tsx` row 21 → `ComposerModel session={null}`.
- `effortEverywhere.test.ts`: keep its floor valid; add `ComposerModel` handling if needed.

## C4. ModelCombobox restyle (pickers builder)
- Trigger becomes the C1 quiet style (no outline), `justify-start`, keeps `id`, `aria-label`, `disabled`, `size`, `className`, `LocalTag`, chevron. `ModelCombobox.test.ts` updated.

## Ownership
- composer builder: `components/chat/Composer.tsx`, `SkillPicker.tsx`, `HeldStrip.tsx`, `VoiceHold.tsx`, `MicButton.tsx`, `McpMenu.tsx`, `QueuedChips.tsx`, their tests, a new pill-row test.
- pickers builder: `ModelMenu.tsx`, `ModelCombobox.tsx`, `ModelPicker.tsx`, `Header.tsx`, `lib/header.ts`, `NewChatPane.tsx`, `notebooks/ChatColumn.tsx`, `notes/NoteChat.tsx`, `dev/effort-fixture.tsx`, `slash.ts`, `SessionMenu.tsx`, every `ModelCombobox` call site if a prop must change, and their tests.
