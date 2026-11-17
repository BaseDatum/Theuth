import { afterEach, expect, test, vi } from 'vitest'
import { canPopout, noteOpenMode, popoutLabel, popoutMeeting, popoutSession, popoutSize, popoutTarget, popoutUrl } from './popout'

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

test('143 D4: a fresh note pops out with ?new=true ahead of window=; the flag never reaches the label or the parsed target', () => {
  expect(popoutUrl({ kind: 'note', doc: 'd1', fresh: true })).toBe('/notes/d1?new=true&window=popout-note-d1')
  expect(popoutUrl({ kind: 'note', doc: 'd1' })).toBe('/notes/d1?window=popout-note-d1')
  // Same label with or without `fresh`, so focus-if-open still finds the window.
  expect(popoutLabel({ kind: 'note', doc: 'd1', fresh: true })).toBe('popout-note-d1')
  expect(popoutTarget('tauri://localhost/notes/d1?new=true&window=popout-note-d1')).toEqual({ kind: 'note', doc: 'd1' })
  // The meeting-note url is unchanged by the general `&` rule.
  expect(popoutUrl({ kind: 'meeting-note', meeting: 'm1' })).toBe('/meetings/m1?pane=note&window=popout-meeting-note-m1')
})

test('144 wave 2: a notebook pops out at /notebooks/$id with label popout-notebook-<id>; the window is landscape, clamped to the screen', () => {
  const t = { kind: 'notebook', notebook: 'nb1' } as const
  expect(popoutUrl(t)).toBe('/notebooks/nb1?window=popout-notebook-nb1')
  expect(popoutLabel(t)).toBe('popout-notebook-nb1')
  expect(popoutTarget('tauri://localhost/notebooks/nb1?window=popout-notebook-nb1')).toEqual(t)
  // A reload keeps the target: the route state is the URL, nothing from the main window.
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutTarget('/notebooks?window=x')).toBeNull()
  expect(popoutSession(t)).toBeNull()
  expect(popoutMeeting(t)).toBeNull()
  expect(popoutSize(t, { availWidth: 1920, availHeight: 1080 })).toEqual({ width: 1100, height: 720 })
  expect(popoutSize(t, { availWidth: 1024, availHeight: 640 })).toEqual({ width: 1024, height: 640 })
  expect(popoutSize({ kind: 'note', doc: 'd1' }, { availWidth: 1920, availHeight: 1080 })).toBeNull()
})

test('143 D3: a new note opens in its own window only on a desktop or browser that can; a phone or iOS navigates', () => {
  expect(noteOpenMode(false, true)).toBe('popout')
  expect(noteOpenMode(true, true)).toBe('navigate')
  expect(noteOpenMode(false, false)).toBe('navigate')
  expect(noteOpenMode(true, false)).toBe('navigate')
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

test('a talk view pops out at /talk/$session, a separate label from the chat of the same session, and closes with it', () => {
  const t = { kind: 'talk', session: '01J8ZK-abc' } as const
  expect(popoutLabel(t)).toBe('popout-talk-01J8ZK-abc')
  expect(popoutUrl(t)).toBe('/talk/01J8ZK-abc?window=popout-talk-01J8ZK-abc')
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutSession(t)).toBe('01J8ZK-abc')
})

test('a sandbox VM desktop pops out at /vm/$name with the contract label popout-vm-<name> (79 §6.2); no session closes it', () => {
  const t = { kind: 'vm', name: 'lab' } as const
  expect(popoutLabel(t)).toBe('popout-vm-lab')
  expect(popoutUrl(t)).toBe('/vm/lab?window=popout-vm-lab')
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutSession(t)).toBeNull()
  expect(popoutTarget('/vm/lab')).toBeNull()
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

test('a sidebar page pops out alone at its own route; the `popout-page-` label is what marks the window, and it round-trips', () => {
  for (const [path, label] of [
    ['/costs', 'popout-page-costs'],
    ['/attention', 'popout-page-attention'],
    ['/artifacts', 'popout-page-artifacts'],
    ['/settings/appearance', 'popout-page-settings_appearance'],
    ['/meetings/new', 'popout-page-meetings_new'],
  ] as const) {
    const t = { kind: 'page', path } as const
    expect(popoutLabel(t)).toBe(label)
    expect(popoutUrl(t)).toBe(`${path}?window=${label}`)
    expect(popoutTarget(popoutUrl(t))).toEqual(t)
    expect(popoutTarget(`tauri://localhost${popoutUrl(t)}`)).toEqual(t)
    expect(popoutSession(t)).toBeNull()
    expect(popoutMeeting(t)).toBeNull()
  }
  // Navigating inside the window keeps it a page window: the label, not the route, says so.
  expect(popoutTarget('/agents/maya?window=popout-page-agents')).toEqual({ kind: 'page', path: '/agents/maya' })
  // Without the page label a route keeps its own reading: an id route is its thing, a bare page route nothing.
  expect(popoutTarget('/artifacts/toolu_01?window=popout-artifact-toolu_01')).toEqual({ kind: 'artifact', callId: 'toolu_01' })
  expect(popoutTarget('/costs?window=x')).toBeNull()
  // A keyed page (New chat, per press) gets its own label; the window still reads as that page.
  const fresh = { kind: 'page', path: '/new', key: 'a1b2c3d4' } as const
  expect(popoutLabel(fresh)).toBe('popout-page-new-a1b2c3d4')
  expect(popoutUrl(fresh)).toBe('/new?window=popout-page-new-a1b2c3d4')
  expect(popoutTarget(popoutUrl(fresh))).toEqual({ kind: 'page', path: '/new' })
})
