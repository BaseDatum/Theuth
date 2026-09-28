import { afterEach, expect, test, vi } from 'vitest'
import { canPopout, popoutLabel, popoutMeeting, popoutSession, popoutTarget, popoutUrl } from './popout'

afterEach(() => vi.unstubAllGlobals())

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

test('a PTY pops out at /pty/$session/$job; its label names both ids and its session closes it', () => {
  const t = { kind: 'pty', session: '01J8ZK-abc', job: 'j.1' } as const
  expect(popoutLabel(t)).toBe('popout-pty-01J8ZK-abc-j_1')
  expect(popoutUrl(t)).toBe('/pty/01J8ZK-abc/j.1?window=popout-pty-01J8ZK-abc-j_1')
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutTarget(popoutUrl({ kind: 'pty', session: 'a/b', job: 'c d' }))).toEqual({ kind: 'pty', session: 'a/b', job: 'c d' })
  // A session without a job is not a terminal window.
  expect(popoutTarget('/pty/s1?window=x')).toBeNull()
  expect(popoutTarget('/pty/s1/j1')).toBeNull()
  // `SessionDeleted` closes a chat's window and a PTY's; a meeting's or note's is not a session.
  expect(popoutSession(t)).toBe('01J8ZK-abc')
  expect(popoutSession({ kind: 'chat', session: 's1' })).toBe('s1')
  expect(popoutSession({ kind: 'meeting', meeting: 'm1' })).toBeNull()
  expect(popoutSession(null)).toBeNull()
})

test('a CodeMap pops out at /codemap/$session/$root; the root (a path, spaces allowed) survives the url and the label is Tauri-safe', () => {
  const t = { kind: 'codemap', session: '01J8ZK-abc', root: '/Users/sky/My Code/theuth' } as const
  expect(popoutLabel(t)).toBe('popout-codemap-01J8ZK-abc-_Users_sky_My_Code_theuth')
  expect(popoutUrl(t)).toBe('/codemap/01J8ZK-abc/%2FUsers%2Fsky%2FMy%20Code%2Ftheuth?window=popout-codemap-01J8ZK-abc-_Users_sky_My_Code_theuth')
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  // A session without a root is not a map window; `SessionDeleted` closes it like a PTY's.
  expect(popoutTarget('/codemap/s1?window=x')).toBeNull()
  expect(popoutSession(t)).toBe('01J8ZK-abc')
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

test('canPopout: a desktop shell or a plain browser has second windows; the iOS shell never does, whatever its width', () => {
  const shell = (tauri: boolean, userAgent: string, maxTouchPoints = 0) => {
    vi.stubGlobal('window', tauri ? { __TAURI_INTERNALS__: {}, innerWidth: 390 } : { innerWidth: 390 })
    vi.stubGlobal('navigator', { userAgent, maxTouchPoints })
  }
  const mac = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)'
  shell(true, mac)
  expect(canPopout()).toBe(true)
  shell(true, 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36')
  expect(canPopout()).toBe(true)
  // A narrow browser tab still has `window.open`.
  shell(false, 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148', 5)
  expect(canPopout()).toBe(true)
  // The iOS shell: iPhone UA, and the iPad's Macintosh UA told apart by touch points.
  shell(true, 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148', 5)
  expect(canPopout()).toBe(false)
  shell(true, mac, 5)
  expect(canPopout()).toBe(false)
})
