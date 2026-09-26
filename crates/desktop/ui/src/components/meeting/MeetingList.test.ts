import { expect, test, vi } from 'vitest'
import { deleteMeeting } from './MeetingList'

test('the row\u2019s Delete asks first: declined → no RPC and `false`; confirmed → `meeting_delete(id)` and `true`', async () => {
  const api = { meeting_delete: vi.fn(async () => null) }
  vi.stubGlobal('confirm', () => false)
  expect(await deleteMeeting(api, 'm1')).toBe(false)
  expect(api.meeting_delete).not.toHaveBeenCalled()
  vi.stubGlobal('confirm', () => true)
  expect(await deleteMeeting(api, 'm1')).toBe(true)
  expect(api.meeting_delete).toHaveBeenCalledWith('m1')
  // The engine's refusal (a meeting still recording) surfaces to the caller's error toast, not as a silent `false`.
  api.meeting_delete.mockRejectedValueOnce(new Error('that meeting is recording; stop it first'))
  await expect(deleteMeeting(api, 'm1')).rejects.toThrow('recording')
  vi.unstubAllGlobals()
})
