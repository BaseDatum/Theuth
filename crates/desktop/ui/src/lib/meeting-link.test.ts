import { expect, test, vi } from 'vitest'
import { meetingForSession, openMeeting } from './meeting-link'
import type { MeetingSummary } from '@/rpc/bindings/MeetingSummary'

const row = (id: string, session: string): MeetingSummary =>
  ({ id, session, started_at: '', ended_at: null, title: null, process_name: 'Zoom', duration_ms: null, state: 'Done' }) as MeetingSummary

test('a chat gets the meeting items only when a meeting names it as its session', () => {
  const list = [row('m1', 's1'), row('m2', 's2')]
  expect(meetingForSession(list, 's2')).toBe('m2')
  expect(meetingForSession(list, 's3')).toBeNull()
  // The list not loaded yet (or no meeting plugin): hidden, not an error.
  expect(meetingForSession(undefined, 's1')).toBeNull()
  expect(meetingForSession([], 's1')).toBeNull()
})

test('⌥ on the selecting click pops the meeting out; a plain click or a key opens it here', () => {
  const here = vi.fn()
  const select = (originalEvent?: unknown) => new CustomEvent('select', { detail: { originalEvent } })
  openMeeting('m1', select({ altKey: false }), here)
  openMeeting('m1', select({ key: 'Enter' }), here)
  openMeeting('m1', select(), here)
  expect(here).toHaveBeenCalledTimes(3)
  // Not in Tauri here, so the popout is a named `window.open`.
  const open = vi.fn()
  vi.stubGlobal('window', { open })
  openMeeting('m1', select({ altKey: true }), here)
  expect(here).toHaveBeenCalledTimes(3)
  expect(open).toHaveBeenCalledWith('/meetings/m1?window=popout-meeting-m1', 'popout-meeting-m1')
  vi.unstubAllGlobals()
})
