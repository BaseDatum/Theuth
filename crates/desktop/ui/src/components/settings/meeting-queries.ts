// Settings › Meetings: the engine's `meeting` block (158 §6.2) that Defaults, Detection and Live all edit. Each
// control writes the whole block with one field changed, never a partial.

import { useQuery } from '@tanstack/react-query'
import { useAction } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import type { MeetingSettings } from '@/rpc/bindings/MeetingSettings'

export const SETTINGS_KEY = ['meeting-settings'] as const

/** The block (`settings.data` as `block`), its whole-block `save`, and `set(patch)` for one control; `busy` until it is read and while a write runs. */
export function useMeetingBlock() {
  const api = useApi()
  const settings = useQuery({ queryKey: SETTINGS_KEY, queryFn: () => api.meeting_settings_get() })
  // The block carries `live_diarization` too, so a save refreshes Live's speaker-labels switch.
  const save = useAction((next: MeetingSettings) => api.meeting_settings_set(next), { keys: [SETTINGS_KEY, queryKeys.meetingLiveDiarization] })
  const block = settings.data
  const set = (patch: Partial<MeetingSettings>) => block && save.mutate({ ...block, ...patch })
  return { settings, block, save, set, busy: !block || save.isPending }
}
