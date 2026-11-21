# 201 as built — PU (the settings UI)

The UI of 201 §12.2 / D15 over the wire fields PC landed. TypeScript/React only, under `crates/desktop/ui`.

## Landed

- **`src/lib/endless.ts`**
  - `ENDLESS_DEFAULTS` gains the 14 keys of §12.1 with their defaults (the placeholders and values while a key is absent).
  - `TAIL_RANGES`, `tailMark(mode, v)` ("64 KB" / "25 %"), `tailPairError(mode, low, high)`: the §12.1 ranges and the high-above-low rule in the UI's words, used by the page, the agent fields and the wizard.
  - `scaledMarks(lowShare, highShare, contextTokens)`: `tail_marks` estimated from a model's window (×3 bytes per token, clamp 8–512 KB, scale down past half the allowance).
  - `autoProfile(model)`: what `profile: auto` resolves to (§7.1: local or ≤ 32 k tokens → small) with its reason.
- **`src/components/settings/shared.tsx`**: `Range` moved here from `Memory.tsx` (unchanged) so tests can capture it; `Count` gains `blank` (the value an emptied field writes) and a string `placeholder`.
- **`src/components/settings/Memory.tsx`** (Settings › Agent › Context)
  - View: *Settle seconds* removed; *Start merging at* (range 50–100 %, `merge_at`).
  - Tail (new, under View): *Tail sizing* select (`tail_mode`); fixed → *Kept after a trim* / *Trim at* KB ranges with token estimates; scaled → the same in percent with the resolved KB for the default model; *Trim early when idle* (`cold_after_secs`, empty = automatic = 0). The pair is checked live while dragging and again on release; a refused pair is never written and the reason shows in a `role="alert"` line; an engine refusal mentioning `tail_` shows there too (and toasts, as every save error does).
  - Compactor: *Profile* select with "Automatic · small, because …" for the summary model in force; *Describe images* switch (`auto`/`off`). Advanced help of `compactor_ctx_bytes`, `batch_units`, `jobs` says "Standard profile only".
  - Ledger (new): *Keep a state ledger*, *Ledger size* (KB), *Show summary-model suggestions* (disabled while the ledger is off).
  - Recall (new): *Prefetch size* (bytes, 0 = off), *Keep zoomed lines unmerged* (`age_by_use`).
- **`src/components/chat/MemoryPanel.tsx`**
  - `Live` / `LIVE`: an array of renderers under the stats line — tail bar (`role="meter"`, tick at the low mark; text only while the marks are 0), generation, lines on a draft, profile and reason. Later phases append entries to `LIVE`.
  - `TailSize` beside `ViewSize`: presets per mode (`TAIL_PRESETS`) plus the chat's own override if it is not a preset; "Use the default" sends `null`. `tailMutation` calls `set_session_tail`.
- **`src/components/agents/RuntimeAccessFields.tsx`** (agent page and hire wizard share `RuntimeFields`): *Context tail* (Use default / Fixed / Scaled), the chosen mode's two marks (KB or %; empty = default; other mode's marks cleared on a mode change), *Compactor profile*. `wizard.ts`: `tailError(d)` and a `runtime` case in `validate`.
- **`src/dev/endless-fixture.ts`**: new status and config fields, `set_session_tail`.
- Tests: `Memory.test.ts` (every §12.1 key's control patches its field, pair validation, mode switch shows the right pair, profile resolution / ledger gating), `MemoryPanel.test.ts` (live values, override trigger, clear sends null), `RuntimeAccessFields.test.ts` (per-agent pair, validation, file keys).

## Deviations

- The per-agent `view_bytes` is not offered anywhere in the UI today, so the per-agent tail and profile keys went beside the agent's *Context summary model* in `RuntimeFields` (agent page and hire wizard both).
- Scaled-mode KB on the settings page is a UI estimate from the default model's window (×3 bytes/token). There is no RPC that returns the engine's turn allowance; when one exists, replace `scaledMarks`.
- Automatic's resolution on the settings page is computed in the UI (`autoProfile`) from the summary model in force; per session the panel shows the engine's `profile`/`profile_reason`.
- The per-session override is a preset list (as `ViewSize` is), not free entry; a stored non-preset override shows as its own row.
- `set_session_tail` is not added to `memoryRequirements`: adding it would hide the whole panel on engines without it.
- `raw_bytes` (retired for chats) keeps its Advanced row unchanged.
- The draft memory sheet (`DraftMemory`, new chat before its first message) has no tail control.

## Seams / later

- Ledger counts, zooms this generation, pending tool-list change: append to `LIVE` in `MemoryPanel.tsx`.
- Engine-side turn allowance for the scaled preview: `scaledMarks` in `lib/endless.ts`.
