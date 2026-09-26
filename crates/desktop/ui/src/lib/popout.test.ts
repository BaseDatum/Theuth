import { expect, test } from 'vitest'
import { popoutLabel, popoutMeeting, popoutTarget, popoutUrl } from './popout'

test('a window opened with ?window= shows only its chat or artifact; anything else is the ordinary shell', () => {
  expect(popoutTarget('tauri://localhost/c/abc?window=popout-chat-abc')).toEqual({ kind: 'chat', session: 'abc' })
  expect(popoutTarget('http://localhost:1420/artifacts/toolu_01?window=popout-artifact-toolu_01')).toEqual({ kind: 'artifact', callId: 'toolu_01' })
  // Encoded ids come back as the id.
  expect(popoutTarget('/c/a%2Fb?window=x')).toEqual({ kind: 'chat', session: 'a/b' })
  // No query: the main window at that route.
  expect(popoutTarget('tauri://localhost/c/abc')).toBeNull()
  expect(popoutTarget('/c/abc?seq=3')).toBeNull()
  // Routes without a popped form, or no id.
  expect(popoutTarget('/settings/general?window=x')).toBeNull()
  expect(popoutTarget('/c/?window=x')).toBeNull()
  expect(popoutTarget('/artifacts?window=x')).toBeNull()
})

test('a note or a meeting pops out too; `?pane=note` on a meeting is its note editor alone', () => {
  expect(popoutTarget('tauri://localhost/notes/d1?window=popout-note-d1')).toEqual({ kind: 'note', doc: 'd1' })
  expect(popoutTarget('/meetings/m1?window=popout-meeting-m1')).toEqual({ kind: 'meeting', meeting: 'm1' })
  expect(popoutTarget('/meetings/m1?pane=note&window=popout-meeting-note-m1')).toEqual({ kind: 'meeting-note', meeting: 'm1' })
  // Another pane value is the whole meeting; no id is nothing.
  expect(popoutTarget('/meetings/m1?pane=chat&window=x')).toEqual({ kind: 'meeting', meeting: 'm1' })
  expect(popoutTarget('/notes?window=x')).toBeNull()
  expect(popoutTarget('/meetings/?window=x')).toBeNull()
  for (const t of [{ kind: 'note', doc: 'a b' } as const, { kind: 'meeting', meeting: 'm/1' } as const, { kind: 'meeting-note', meeting: 'm 2' } as const]) {
    expect(popoutTarget(popoutUrl(t))).toEqual(t)
    expect(popoutUrl(t).endsWith(`window=${popoutLabel(t)}`)).toBe(true)
  }
  expect(popoutLabel({ kind: 'meeting-note', meeting: 'm 2' })).toBe('popout-meeting-note-m_2')
  // Which popped windows a deleted meeting takes with it.
  expect(popoutMeeting({ kind: 'meeting', meeting: 'm1' })).toBe('m1')
  expect(popoutMeeting({ kind: 'meeting-note', meeting: 'm1' })).toBe('m1')
  expect(popoutMeeting({ kind: 'chat', session: 'm1' })).toBeNull()
  expect(popoutMeeting(null)).toBeNull()
})

test('the label names the target with only the characters a Tauri window label allows, and the url carries it back', () => {
  expect(popoutLabel({ kind: 'chat', session: '01J8ZK-abc' })).toBe('popout-chat-01J8ZK-abc')
  expect(popoutLabel({ kind: 'artifact', callId: 'call.1/2 x' })).toBe('popout-artifact-call_1_2_x')
  for (const t of [{ kind: 'chat', session: 'a b' } as const, { kind: 'artifact', callId: 'c/d' } as const]) {
    const url = popoutUrl(t)
    expect(url.endsWith(`?window=${popoutLabel(t)}`)).toBe(true)
    // The url round-trips to the target it was built from.
    expect(popoutTarget(url)).toEqual(t)
  }
})
