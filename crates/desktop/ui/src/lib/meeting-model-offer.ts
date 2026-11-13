// 158 §6.4 (W1): the one-time offer on Settings › Meetings — the engine has no `meeting.model` yet, but the start
// form remembered one in `localStorage['meeting-settings'].model`: one toast, one RPC. Shown once either way; the
// answer lives under its own key beside `meeting-settings` (that store's fields are untouched).

import { toast } from 'sonner'
import type { MeetingSettings } from "@/rpc/bindings/method.meeting.output"

/** Beside `lib/meeting-settings.ts`'s `meeting-settings`; any value = already offered. */
export const OFFERED_KEY = 'meeting-settings-model-offered'

type Storage = Pick<globalThis.Storage, 'getItem' | 'setItem'>

/**
 * Offer once; `true` when the toast was shown. `label` is the model's display name (falls back to `saved`).
 * The action writes the whole block with `model: saved` (§6.2: never a partial write).
 */
export function offerMeetingDefault({
  block,
  saved,
  label,
  write,
  storage = localStorage,
  notify = toast,
}: {
  block: MeetingSettings
  saved: string | null
  label?: string
  write(next: MeetingSettings): void
  storage?: Storage
  notify?: Pick<typeof toast, 'message'>
}): boolean {
  if (block.model !== null || saved === null || storage.getItem(OFFERED_KEY) !== null) return false
  storage.setItem(OFFERED_KEY, new Date().toISOString())
  notify.message(`Use ${label ?? saved} as the meeting default?`, {
    description: 'The start form remembered it; the engine has no meeting model set yet. Recorded prompts and companions would use it too.',
    duration: Infinity,
    action: { label: 'Use it', onClick: () => write({ ...block, model: saved }) },
  })
  return true
}
