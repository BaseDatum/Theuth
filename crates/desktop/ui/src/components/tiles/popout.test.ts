import { createContext, createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Popout from '@/lib/popout'
import type { Tile } from '@/lib/tiles'

// Every tile kind's header must carry Open in new window. `TileFrame` is replaced by a probe that records the `popOut`
// it was handed; the kinds' bodies are stubs (what they draw is not under test, only the header contract).
const probe = vi.hoisted(() => ({ popOut: undefined as (() => unknown) | undefined, opened: [] as unknown[] }))

vi.mock('@/components/tiles/TileFrame', () => ({
  TileAlone: createContext(false),
  TileFrame: (p: { title: string; popOut?: () => unknown }) => {
    probe.popOut = p.popOut
    return createElement('header', { 'data-title': p.title, 'data-popout': p.popOut ? 'yes' : 'no' })
  },
}))
vi.mock('@/lib/popout', async (original) => ({
  ...(await original<typeof Popout>()),
  canPopout: () => true,
  openPopout: async (t: unknown) => {
    probe.opened.push(t)
  },
}))

const Stub = ({ children }: { children?: ReactNode }) => children ?? null
const query = { data: undefined, refetch() {}, isFetching: false }
vi.mock('@/rpc/capabilities', () => ({ Capability: Stub, Unavailable: Stub, useSupported: () => true }))
vi.mock('@/components/chat/ArtifactPanel', async () => {
  const { artifactPopout } = await import('@/components/chat/ArtifactCard')
  return {
    dismissArtifact() {},
    // The real `ArtifactContent` hands `popOut: artifactPopout(shown)`; the stub hands the same for an image artifact.
    ArtifactContent: ({ children }: { children(p: unknown): ReactNode }) =>
      children({ title: 'chart.png', meta: '', controls: null, body: null, popOut: artifactPopout({ call_id: 'c1', session: 's1', kind: 'image' } as never) }),
  }
})
vi.mock('@/components/chat/ArtifactCard', () => ({
  artifactPopout: (a: { call_id: string }) => async () => void probe.opened.push({ kind: 'artifact', callId: a.call_id }),
}))
vi.mock('@/components/chat/BrowserLive', () => ({ BrowserScreen: Stub }))
vi.mock('@/components/codemap/CodeMapPanel', async () => {
  const { openPopout } = await import('@/lib/popout')
  return {
    CodeMapPanel: ({ chrome, session, root }: { chrome(p: unknown): ReactNode; session: string; root: string }) =>
      chrome({ meta: '', controls: null, body: null, popOut: () => openPopout({ kind: 'codemap', session, root }) }),
  }
})
vi.mock('@/components/codemap/requirements', () => ({ requirements: {} }))
vi.mock('@/components/vm/requirements', () => ({ requirements: {} }))
vi.mock('@/components/vm/VmDesktop', () => ({ VmDesktop: Stub }))
vi.mock('@/components/chat/MemoryPanel', () => ({ DRAFT_NOTE: '', DraftMemoryBody: Stub, MemoryBody: Stub }))
vi.mock('@/components/code/DiffBody', () => ({
  DiffBody: Stub,
  DiffLayoutToggle: Stub,
  diffMeta: () => '',
  diffTitle: () => 'diff',
  useDiffLayout: () => ({ layout: 'unified', choose() {}, ref: null }),
  useDiffView: () => query,
}))
vi.mock('@/components/code/FileView', () => ({
  FileView: Stub,
  fileMeta: () => '',
  highlightedLines: () => 0,
  langOf: () => '',
  linesOf: () => [],
  useFileFollow: () => ({ deleted: false }),
  useFileView: () => query,
}))
vi.mock('@/components/chat/DiagramFrame', () => ({ HOME: {}, Surface: Stub, Tool: Stub, Tools: Stub }))
vi.mock('@/components/chat/JobsSheet', () => ({ JobFilters: Stub, JobList: Stub, LiveJob: Stub, useJobs: () => ({ summary: '' }) }))
vi.mock('@/components/notes/NotePage', () => ({ NoteView: Stub }))
vi.mock('@/components/notes/requirements', () => ({ panelRequirements: {} }))
vi.mock('@/components/chat/Attachments', () => ({ Original: Stub }))
vi.mock('@/components/chat/MessageAssistant', () => ({ RemoveButton: Stub, rehypePlugins: [] }))
vi.mock('@/components/plugins/PluginSurface', () => ({ PluginSurface: Stub }))
vi.mock('@/routeTree.gen', () => ({ routeTree: {} }))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  RouterProvider: Stub,
  createRouter: () => ({ subscribe: () => () => {}, state: { location: { href: '/tasks' } }, navigate: async () => {} }),
  createMemoryHistory: () => ({ push: () => {}, replace: () => {} }),
  useNavigate: () => async () => {},
}))
vi.mock('@/components/chat/SessionMenu', () => ({ PROMPT_NOTE: '', PromptBody: Stub }))
vi.mock('@/components/library/Skills', () => ({ SkillBody: Stub }))
vi.mock('@/components/tasks/graph/TaskGraph', () => ({ TaskGraph: Stub }))
vi.mock('@/components/chat/TerminalViewer', () => ({
  FocusWindowButton: Stub,
  Terminal: Stub,
  TerminalScreen: Stub,
  TerminalSelect: Stub,
  usePoppedElsewhere: () => false,
  useTerminals: () => [{ session: 's1', id: 'j1' }],
}))
vi.mock('@/components/whiteboard/WhiteboardView', () => ({ WhiteboardView: Stub }))

const { TILE_KINDS, PoppedTile } = await import('./registry')
const { popoutTarget, popoutUrl, stashTileProps } = await import('@/lib/popout')

/** A sample payload per kind, as its opener (`open.ts`) builds it. */
const SAMPLES: Record<string, unknown> = {
  artifact: { callId: 'c1' },
  browser: {},
  codemap: { root: '/repo' },
  diagram: { svg: `<svg>${'<path d="M0 0"/>'.repeat(400)}</svg>`, name: 'a1b2', label: 'Flow' },
  diff: { session: 's1', id: 'd1', title: 'Staged', spec: { mode: 'staged', root: '/repo' } },
  file: { session: 's1', path: '/repo/a b.ts', id: 'f1', title: 'a b.ts', line: 3 },
  jobs: {},
  'job-log': { session: 's1', job: 'j1', label: 'build' },
  'job-terminal': { session: 's1', job: 'j2', label: 'vim' },
  media: { item: { key: 'o0', name: 'chart.png', path: '/tmp/chart.png', image: true, pdf: false }, session: 's1', active: true },
  memory: {},
  note: { doc: 'd1', title: 'Plan' },
  plugin: { plugin: 'demo', placement: 'panel' },
  prompt: {},
  skill: { name: 'pdf' },
  'tasks-graph': { project: 'theuth' },
  terminals: {},
  view: { path: '/tasks', title: 'Tasks' },
  vm: { name: 'box' },
  whiteboard: { doc: 'w1', title: 'Sketch' },
}
/** The window each kind opens: a dedicated route where one exists, else the generic tile window. */
const WINDOW: Record<string, string> = {
  artifact: 'artifact',
  codemap: 'codemap',
  diff: 'diff',
  file: 'file',
  'job-terminal': 'pty',
  // The `/tasks/graph` page window: navigable, one per project.
  'tasks-graph': 'page',
  note: 'note',
  terminals: 'pty',
  view: 'page',
  vm: 'vm',
}

/** A localStorage stand-in shared by "both windows". */
function memoryStorage(): Storage {
  const m = new Map<string, string>()
  return {
    get length() {
      return m.size
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  }
}

afterEach(() => {
  probe.popOut = undefined
  probe.opened = []
  vi.unstubAllGlobals()
})

const tileOf = (kind: string, session = 's1'): Tile => ({ id: `${kind}:${session}:`, kind, session, props: SAMPLES[kind] })

test('every registered tile kind has a sample here (a new kind must be added, and must pop out)', () => {
  expect(Object.keys(TILE_KINDS).sort()).toEqual(Object.keys(SAMPLES).sort())
})

test.each(Object.keys(SAMPLES))('the %s tile header carries Open in new window, and it opens a window that shows the same thing', async (kind) => {
  const store = memoryStorage()
  vi.stubGlobal('localStorage', store)
  const html = renderToStaticMarkup(createElement('div', null, TILE_KINDS[kind]!.render({ tile: tileOf(kind), first: false, close() {} })))
  expect(html).toContain('data-popout="yes"')
  // The window's promise, not fire-and-forget: the frame closes the tile only once it resolved (`popOutAndClose`).
  const opening = probe.popOut?.()
  expect(opening).toBeInstanceOf(Promise)
  await opening
  const target = probe.opened[0] as Popout.PopoutTarget
  expect(target.kind).toBe(WINDOW[kind] ?? 'tile')
  if (target.kind === 'tile') {
    expect(target).toEqual({ kind: 'tile', tile: kind, session: 's1', props: SAMPLES[kind] })
    // What `openPopout` does before opening: a long payload (the diagram's svg) goes to the shared storage.
    stashTileProps(target, store)
  }
  // The window's URL names the same target back (the popped window reads it from there, or from the stash). A page
  // window's key and query live in its label and URL only, never in the parsed target.
  if (target.kind === 'page') expect(popoutTarget(popoutUrl(target))).toEqual({ kind: 'page', path: target.path.split('?')[0] })
  else expect(popoutTarget(popoutUrl(target))).toEqual(target)
})

test("a draft's Context tile pops out too", () => {
  const html = renderToStaticMarkup(createElement('div', null, TILE_KINDS.memory!.render({ tile: tileOf('memory', 'draft:1234'), first: false, close() {} })))
  expect(html).toContain('data-popout="yes"')
})

test('the popped tile window draws the kind over the props; gone props or an unknown kind say so', () => {
  expect(renderToStaticMarkup(createElement(PoppedTile, { kind: 'media', session: 's1', props: SAMPLES.media }))).toContain('data-title="chart.png"')
  expect(renderToStaticMarkup(createElement(PoppedTile, { kind: 'media', session: 's1', props: undefined }))).toContain('data-title="Closed"')
  expect(renderToStaticMarkup(createElement(PoppedTile, { kind: 'nope', session: 's1', props: {} }))).toContain('data-title="Closed"')
})
