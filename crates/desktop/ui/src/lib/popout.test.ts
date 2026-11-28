import { afterEach, expect, test, vi } from 'vitest'
import { type PopoutTarget, TILE_STASH, artifactsRail, canPopout, noteOpenMode, popoutLabel, popoutTitle, popoutMeeting, popoutSession, popoutSize, popoutTarget, popoutUrl, registerNodeTerminalPopout, soloPopout, stashTileProps, takeNodeTerminalPopout, tileProps, newChatPopout } from './popout'

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

// The black Library window: the sidebar's Library button opens `/artifacts` as a page window, and `/artifacts` rendered
// a bare outlet in every popout — with no `$callId` child, nothing at all.
test('/artifacts keeps its Explorer in the main window and the Library page window; only an artifact window shows the viewer alone', () => {
  const library = popoutTarget(popoutUrl({ kind: 'page', path: '/artifacts' }))
  expect(library).toEqual({ kind: 'page', path: '/artifacts' })
  expect(artifactsRail(null)).toBe(true)
  expect(artifactsRail(library)).toBe(true)
  expect(artifactsRail(popoutTarget(popoutUrl({ kind: 'artifact', callId: 'c1' })))).toBe(false)
})

test('a page window is not a solo popout — its pages render as in the main window; every single-subject window is one', () => {
  expect(soloPopout(null)).toBe(false)
  // Navigating inside the Library window (to a note, say) keeps it a page window.
  expect(soloPopout(popoutTarget('/notes/n1?window=popout-page-artifacts'))).toBe(false)
  for (const t of [{ kind: 'artifact', callId: 'c1' }, { kind: 'note', doc: 'n1' }, { kind: 'chat', session: 's1' }, { kind: 'meeting', meeting: 'm1' }, { kind: 'notebook', notebook: 'b1' }] as const) {
    expect(soloPopout(popoutTarget(popoutUrl(t)))).toBe(true)
  }
})

test('205 D6: a web view pops out at /web/$id with its address and title in the query and label popout-web-<id>', () => {
  const t = { kind: 'web', id: '0123456789abcdef', url: 'http://localhost:3000/a?b=1&c=2#d', title: 'Dev & docs' } as const
  expect(popoutLabel(t)).toBe('popout-web-0123456789abcdef')
  expect(popoutUrl(t)).toBe('/web/0123456789abcdef?url=http%3A%2F%2Flocalhost%3A3000%2Fa%3Fb%3D1%26c%3D2%23d&title=Dev%20%26%20docs&window=popout-web-0123456789abcdef')
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutTarget(`tauri://localhost${popoutUrl(t)}`)).toEqual(t)
  expect(soloPopout(t)).toBe(true)
  expect(popoutSession(t)).toBeNull()
  expect(popoutTarget('/web/0123456789abcdef')).toBeNull()
})

test('212: a show_file file pops out at /file/$session with its path and lines in the query; the session closes it', () => {
  const t = { kind: 'file', session: '01J8ZK-abc', path: '/Users/sky/My Code/a&b.rs', line: 12, end_line: 40 } as const
  expect(popoutLabel(t)).toBe('popout-file-01J8ZK-abc-_Users_sky_My_Code_a_b_rs')
  expect(popoutUrl(t)).toBe('/file/01J8ZK-abc?path=%2FUsers%2Fsky%2FMy%20Code%2Fa%26b.rs&line=12&end_line=40&window=popout-file-01J8ZK-abc-_Users_sky_My_Code_a_b_rs')
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutTarget(`tauri://localhost${popoutUrl(t)}`)).toEqual(t)
  // Lines are optional and leave the label alone: the same file is one window wherever it was scrolled to.
  const bare = { kind: 'file', session: 's1', path: '/a.rs' } as const
  expect(popoutUrl(bare)).toBe('/file/s1?path=%2Fa.rs&window=popout-file-s1-_a_rs')
  expect(popoutTarget(popoutUrl(bare))).toEqual(bare)
  expect(popoutLabel({ ...bare, line: 3 })).toBe(popoutLabel(bare))
  expect(popoutTarget(popoutUrl({ kind: 'file', session: 's1', path: '/a.rs', line: 7 }))).toEqual({ kind: 'file', session: 's1', path: '/a.rs', line: 7 })
  // No path is no file window; a bad line is dropped.
  expect(popoutTarget('/file/s1?window=x')).toBeNull()
  expect(popoutTarget('/file/s1?path=%2Fa&line=zero&end_line=0&window=x')).toEqual({ kind: 'file', session: 's1', path: '/a' })
  expect(popoutSession(t)).toBe('01J8ZK-abc')
  expect(soloPopout(t)).toBe(true)
})

test('212: a show_diff diff pops out at /diff/$session with its spec JSON in the query; the label keys on a short hash of it', () => {
  const t = { kind: 'diff', session: '01J8ZK-abc', spec: { mode: 'range', root: '/Users/sky/repo', a: 'a'.repeat(40), b: 'b'.repeat(40), three_dot: true, paths: ['src/a b.rs'] } } satisfies PopoutTarget
  const label = popoutLabel(t)
  expect(label).toMatch(/^popout-diff-01J8ZK-abc-[0-9a-f]{8}$/)
  expect(popoutUrl(t)).toBe(`/diff/01J8ZK-abc?spec=${encodeURIComponent(JSON.stringify(t.spec))}&window=${label}`)
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutTarget(`tauri://localhost${popoutUrl(t)}`)).toEqual(t)
  // Another spec of the same session is another window; the same spec again focuses the open one.
  const staged = { kind: 'diff', session: '01J8ZK-abc', spec: { mode: 'staged', root: '/Users/sky/repo' } } as const
  expect(popoutLabel(staged)).not.toBe(label)
  expect(popoutLabel({ ...t, spec: { ...t.spec } })).toBe(label)
  expect(popoutTarget(popoutUrl(staged))).toEqual(staged)
  // A missing or broken spec is no diff window.
  expect(popoutTarget('/diff/s1?window=x')).toBeNull()
  expect(popoutTarget('/diff/s1?spec=%7Bnope&window=x')).toBeNull()
  expect(popoutTarget('/diff/s1?spec=%5B%5D&window=x')).toBeNull()
  expect(popoutSession(t)).toBe('01J8ZK-abc')
})

/** A localStorage stand-in shared by "both windows". */
function memoryStorage(): Storage & { keys(): string[] } {
  const m = new Map<string, string>()
  return {
    get length() {
      return m.size
    },
    keys: () => [...m.keys()],
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  }
}

test('a generic tile pops out at /tile/$kind/$session with its props JSON in the query; the label keys on a hash of them', () => {
  const t = { kind: 'tile', tile: 'media', session: '01J8-s', props: { item: { name: 'chart.png', path: '/tmp/a b.png', image: true }, active: true } } satisfies PopoutTarget
  const label = popoutLabel(t)
  expect(label).toMatch(/^popout-tile-media-01J8-s-[0-9a-f]{8}$/)
  expect(popoutUrl(t)).toBe(`/tile/media/01J8-s?props=${encodeURIComponent(JSON.stringify(t.props))}&window=${label}`)
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutTarget(`tauri://localhost${popoutUrl(t)}`)).toEqual(t)
  // Other props are another window; the same again focuses the open one. Props-less kinds (`jobs`) round-trip `{}`.
  expect(popoutLabel({ ...t, props: { ...t.props, active: false } })).not.toBe(label)
  const jobs = { kind: 'tile', tile: 'jobs', session: 's 1', props: {} } as const
  expect(popoutTarget(popoutUrl(jobs))).toEqual(jobs)
  expect(popoutTarget('/tile/media?window=x')).toBeNull()
  // The owning session closes it; a draft's tile has no session yet.
  expect(popoutSession(t)).toBe('01J8-s')
  expect(popoutSession({ kind: 'tile', tile: 'memory', session: 'draft:abc', props: {} })).toBeNull()
  expect(soloPopout(t)).toBe(true)
})

test('216: a plugin tile pops out as a generic tile; its target round-trips and its label keys on the placement', () => {
  const t = { kind: 'tile', tile: 'plugin', session: 's1', props: { plugin: 'turns', placement: 'turns' } } satisfies PopoutTarget
  const label = popoutLabel(t)
  expect(label).toMatch(/^popout-tile-plugin-s1-[0-9a-f]{8}$/)
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutLabel({ ...t, props: { plugin: 'turns', placement: 'other' } })).not.toBe(label)
})

test('tile props too long for a URL ride in localStorage: the URL names a stash, the popped window reads it back', () => {
  const store = memoryStorage()
  vi.stubGlobal('localStorage', store)
  const t = { kind: 'tile', tile: 'diagram', session: 's1', props: { svg: `<svg>${'x'.repeat(10_000)}</svg>`, name: 'n', label: 'Flow' } } satisfies PopoutTarget
  const url = popoutUrl(t)
  expect(url).toMatch(/^\/tile\/diagram\/s1\?stash=p[0-9a-f]{8}&window=popout-tile-diagram-s1-[0-9a-f]{8}$/)
  // Not written yet: the window finds nothing (its route says the tile is gone).
  expect(tileProps({ stash: new URL(url, 'http://x').searchParams.get('stash') })).toBeUndefined()
  stashTileProps(t, store)
  expect(popoutTarget(url)).toEqual(t)
  // The router may hand inline props as the parsed object, or the JSON string.
  expect(tileProps({ props: { a: 1 } })).toEqual({ a: 1 })
  expect(tileProps({ props: '{"a":1}' })).toEqual({ a: 1 })
  // Short props are never stashed; old stashes are dropped as new ones come.
  stashTileProps({ kind: 'tile', tile: 'jobs', session: 's1', props: {} }, store)
  for (let i = 0; i < 40; i++) stashTileProps({ ...t, props: { ...t.props, label: `v${i}` } }, store)
  const kept = store.keys().filter((k) => k.startsWith(TILE_STASH))
  expect(kept.length).toBeLessThanOrEqual(16)
  expect(store.getItem(TILE_STASH + new URL(popoutUrl({ ...t, props: { ...t.props, label: 'v39' } }), 'http://x').searchParams.get('stash'))).not.toBeNull()
})

test("a draft pane pops out as a New chat window of its own, keyed on the draft, its project preset kept", () => {
  const t = newChatPopout('draft:0f1e2d3c-aaaa-bbbb-cccc-000000000000?project=web')
  expect(t).toEqual({ kind: 'page', path: '/new', key: '0f1e2d3c' })
  expect(popoutLabel(t)).toBe('popout-page-new-0f1e2d3c')
  const preset = newChatPopout('draft:0f1e2d3c-aaaa', 'my app')
  expect(popoutUrl(preset)).toBe('/new?project=my%20app&window=popout-page-new_project_my_20app-0f1e2d3c')
  // The window is a page window at /new (the preset rides the URL, the router reads it).
  expect(popoutTarget(popoutUrl(preset))).toEqual({ kind: 'page', path: '/new' })
})

test('a window opens named after its subject or kind, never left to the host default', () => {
  expect(popoutTitle({ kind: 'page', path: '/projects' })).toBe('Projects')
  expect(popoutTitle({ kind: 'page', path: '/attention' })).toBe('Inbox')
  expect(popoutTitle(newChatPopout('draft:0f1e2d3c-aaaa', 'my app'))).toBe('New chat')
  expect(popoutTitle({ kind: 'file', session: 's', path: '/repo/src/main.rs', line: 3 })).toBe('main.rs')
  expect(popoutTitle({ kind: 'web', id: 'w', url: 'https://x.dev', title: ' Docs ' })).toBe('Docs')
  expect(popoutTitle({ kind: 'web', id: 'w', url: 'https://x.dev', title: '' })).toBe('Web')
  expect(popoutTitle({ kind: 'vm', name: 'sandbox-1' })).toBe('sandbox-1')
  expect(popoutTitle({ kind: 'node-terminal', engine: 'e1', shell: 'main', name: 'sky-do' })).toBe('sky-do')
  expect(popoutTitle({ kind: 'node-terminal', engine: 'e1', shell: 'main' })).toBe('Terminal')
  expect(popoutTitle({ kind: 'chat', session: 'abc' })).toBe('Chat')
  expect(popoutTitle({ kind: 'tile', tile: 'diagram', session: 's', props: {} })).toBe('Diagram')
})

test('a standalone node terminal pops out at /terminal/$engine/$shell with label popout-terminal-<engine>-<shell> and no session', () => {
  const t: PopoutTarget = { kind: 'node-terminal', engine: 'node-1', shell: 'main', name: 'sky-do' }
  expect(popoutLabel(t)).toBe('popout-terminal-node-1-main')
  expect(popoutUrl(t)).toBe('/terminal/node-1/main?name=sky-do&window=popout-terminal-node-1-main')
  expect(popoutTarget(popoutUrl(t))).toEqual(t)
  expect(popoutTarget('tauri://localhost/terminal/node-1/main?name=sky-do&window=popout-terminal-node-1-main')).toEqual(t)
  // Without a shell id or without `?window=`, not a popout window.
  expect(popoutTarget('/terminal/node-1?window=x')).toBeNull()
  expect(popoutTarget('/terminal/node-1/main')).toBeNull()
  // Independent of any chat session or meeting.
  expect(popoutSession(t)).toBeNull()
  expect(popoutMeeting(t)).toBeNull()
  expect(soloPopout(t)).toBe(true)

  // Close-cleanup registry records and consumes once.
  const map = new Map<string, string>()
  const store = {
    get length() { return map.size },
    key: (i: number) => [...map.keys()][i] ?? null,
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    removeItem: (k: string) => void map.delete(k),
    clear: () => map.clear(),
  } as Storage
  registerNodeTerminalPopout(popoutLabel(t), { engine: 'node-1', shell: 'main' }, store)
  expect(takeNodeTerminalPopout(popoutLabel(t), store)).toEqual({ engine: 'node-1', shell: 'main' })
  expect(takeNodeTerminalPopout(popoutLabel(t), store)).toBeNull()
})

test('a Deck pops out with kind deck preserving section, snapshot, and title across URL roundtrip', async () => {
  const { deckPopoutTarget, routeWindow } = await import('./popout')
  const plain: PopoutTarget = { kind: 'deck', deck: 'deck-1', title: 'Ops Pulse' }
  expect(popoutLabel(plain)).toBe('popout-deck-deck-1')
  expect(popoutUrl(plain)).toBe('/decks/deck-1?window=popout-deck-deck-1')
  expect(popoutTarget(popoutUrl(plain))).toEqual({ kind: 'deck', deck: 'deck-1' })
  expect(popoutTitle(plain)).toBe('Ops Pulse')
  expect(popoutTitle({ kind: 'deck', deck: 'deck-1' })).toBe('Deck')
  expect(soloPopout(plain)).toBe(true)

  const scoped: PopoutTarget = { kind: 'deck', deck: 'deck/1', section: 'sec-cpu', snapshot: 'snap-42', title: 'CPU Deck' }
  const label = popoutLabel(scoped)
  expect(label).toMatch(/^popout-deck-deck_1-[0-9a-f]{8}$/)
  const url = popoutUrl(scoped)
  expect(url).toBe(`/decks/deck%2F1?section=sec-cpu&snapshot=snap-42&window=${label}`)
  expect(popoutTarget(url)).toEqual({ kind: 'deck', deck: 'deck/1', section: 'sec-cpu', snapshot: 'snap-42' })
  expect(popoutTarget(`tauri://localhost${url}`)).toEqual({ kind: 'deck', deck: 'deck/1', section: 'sec-cpu', snapshot: 'snap-42' })
  expect(routeWindow('/decks/deck%2F1?section=sec-cpu&snapshot=snap-42#top')).toEqual({
    kind: 'deck',
    deck: 'deck/1',
    section: 'sec-cpu',
    snapshot: 'snap-42',
  })
  expect(deckPopoutTarget('/decks/deck-1?section=s1&snapshot=snap-9', 'Ops Deck')).toEqual({
    kind: 'deck',
    deck: 'deck-1',
    section: 's1',
    snapshot: 'snap-9',
    title: 'Ops Deck',
  })
  expect(deckPopoutTarget('/decks')).toBeNull()
  expect(popoutTarget('/decks?window=x')).toBeNull()
})
