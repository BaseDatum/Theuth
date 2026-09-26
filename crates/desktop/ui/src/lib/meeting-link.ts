// A chat's meeting: meetings carry their own session id (`MeetingSummary.session`), and the list is the only
// place the UI has that link (no session → meeting RPC), so the chat menus look the session up in the cached
// `meeting_list`. Hidden when no meeting claims the session.

import { useQuery } from '@tanstack/react-query'
import { canPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import type { MeetingSummary } from '@/rpc/bindings/MeetingSummary'

/** The meeting whose own chat is `session`, else `null`. */
export const meetingForSession = (meetings: readonly MeetingSummary[] | undefined, session: string): string | null =>
  meetings?.find((m) => m.session === session)?.id ?? null

export function useMeetingForSession(session: string): string | null {
  const api = useApi()
  const list = useQuery({ queryKey: queryKeys.meetings, queryFn: () => api.meeting_list(null) })
  return meetingForSession(list.data, session)
}

/**
 * The "Open meeting" item: ⌥ on the selecting click opens the meeting in its own window, anything else opens it
 * here through `here`. Wired to the item's `onClick` (not `onSelect`, whose event carries no modifiers): Radix
 * routes Enter/Space to `click()` too, with a modifier-less event. Where there are no second windows
 * (`canPopout()`), ⌥ is a plain open.
 */
export function openMeeting(meeting: string, alt: boolean, here: () => void): void {
  if (alt && canPopout()) void openPopout({ kind: 'meeting', meeting })
  else here()
}
