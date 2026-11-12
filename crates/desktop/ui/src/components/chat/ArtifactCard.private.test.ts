import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type * as React from 'react'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as PageTheme from '@/lib/page-theme'
import { usePrivate } from '@/lib/private'
import type * as Rpc from '@/lib/rpc'
import { useEngines } from '@/link/engines'
import type { Artifact } from "@/rpc/bindings/method.artifact.output"
import { type Entry, useSessions } from '@/store/sessions'
import { active } from '@/store/engine-scope'
import { ArchiveView } from '@/components/artifacts/ArchiveView'
import { ArtifactCard, openShow, saveAs } from './ArtifactCard'

// Server rendering never runs effects, and the card's body read (`useText`) is one. Run each effect once in a
// microtask queued by the render, so the read reaches the api; an effect that needs a real DOM throws and is dropped.
vi.mock('react', async (original) => ({
  ...(await original<typeof React>()),
  useEffect: (fn: () => void) =>
    queueMicrotask(() => {
      try {
        fn()
      } catch {
        // no DOM here
      }
    }),
}))

// The engine's rule (51 §4.2): a scratch path is served only when its session is named; a durable one session-less.
// Records (`p-…` call ids are the private tree's) follow it too: a private one only to its owner naming the session.
const api = vi.hoisted(() => {
  const owned = (id: string, session?: string) => {
    if (id.startsWith('p-') && session !== 'private-root') throw new Error('unauthorized')
  }
  return {
    get_attachment: vi.fn(async (path: string, session?: string) => {
      if (path.includes('/theuth-private-') !== (session !== undefined)) throw new Error(`${path}: not an attachment`)
      return btoa(`<h1>${path}</h1>`)
    }),
    get_artifact: vi.fn(async (call_id: string, session?: string) => {
      owned(call_id, session)
      return { id: `id-${call_id}`, session: session ?? 'durable', call_id, name: 'a.md', title: 'a', kind: 'markdown', mime: 'text/markdown', bytes: 1, version: 1, path: '/x', source: null, preview: null, pages: null, origin: null, created_at: 0 } satisfies Artifact
    }),
    get_artifact_listing: vi.fn(async (call_id: string, session?: string) => {
      owned(call_id, session)
      return { entries: [], total_entries: 0, total_size: 0, truncated: false }
    }),
    export_artifact: vi.fn(async (id: string, dest: string, session?: string) => {
      owned(id.replace(/^id-/, ''), session)
      return dest
    }),
  }
})
const save = vi.hoisted(() => vi.fn(async () => '/tmp/out/a.md'))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save }))
vi.mock('@/components/hud/queries', () => ({ useMarkShown: () => {} }))
// The page frame reads the document's computed theme.
vi.mock('@/lib/page-theme', async (actual) => ({ ...(await actual<typeof PageTheme>()), useHtmlPageTheme: () => ({ appearance: 'dark', variables: {} }) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

const SCRATCH = `/var/folders/x/T/theuth-private-${'5e'.repeat(16)}`
const ROOT = 'private-root'

// An owned tree in every test: in the durable one it must not leak its session into a durable read.
beforeEach(() => usePrivate.setState({ current: { engine: useEngines.getState().active, root: ROOT, session: ROOT, secret: '' } }))
afterEach(() => {
  vi.clearAllMocks()
  usePrivate.getState().clear()
})

type Tool = Extract<Entry, { kind: 'tool' }>
/** The card for one shown artifact of `session`, rendered; resolves once its effects ran. */
async function card(session: string, kind: Artifact['kind'], path: string): Promise<string> {
  const name = path.slice(path.lastIndexOf('/') + 1)
  useSessions.getState().setArtifacts(session, [
    { id: name, session, call_id: `c-${name}`, name, title: name, kind, mime: 'text/plain', bytes: 10, version: 1, path, source: null, preview: null, pages: null, origin: null, created_at: 0 },
  ])
  const tool: Tool = { id: name, kind: 'tool', callId: `c-${name}`, name: 'show', args: { path }, pending: false, child: false, result: { output: `shown ${name}`, isError: false, files: [] } }
  // Server rendering reads zustand's initial state: make the sessions store answer its live one.
  const store = active().sessions
  Object.assign(store, { getInitialState: store.getState })
  const html = renderToStaticMarkup(
    createElement(QueryClientProvider, { client: new QueryClient() }, createElement(TooltipProvider, null, createElement(ArtifactCard, { tool, session }))),
  )
  // The effects' microtasks were queued by the render, so they run before this continuation.
  await Promise.resolve()
  return html
}

test('html and markdown cards in a private chat read their bytes naming the session; no pop-out window', async () => {
  for (const [kind, path] of [
    ['html', `${SCRATCH}/artifacts/page.html`],
    ['markdown', `${SCRATCH}/artifacts/notes.md`],
  ] as const) {
    const html = await card(ROOT, kind, path)
    expect(api.get_attachment).toHaveBeenCalledWith(path, ROOT)
    // A second window is another peer, which the engine would refuse.
    expect(html).not.toContain('Open in new window')
  }
})

test('the same cards in a durable chat keep the one-argument read and the pop-out', async () => {
  for (const [kind, path] of [
    ['html', '/data/artifacts/page.html'],
    ['markdown', '/data/artifacts/notes.md'],
  ] as const) {
    const html = await card('durable', kind, path)
    expect(api.get_attachment.mock.calls.find(([p]) => p === path)).toEqual([path])
    expect(html).toContain('Open in new window')
  }
})

const record = (call_id: string, session: string, kind: Artifact['kind'] = 'markdown'): Artifact => ({
  id: `id-${call_id}`, session, call_id, name: 'a.md', title: 'a', kind, mime: 'text/markdown', bytes: 1, version: 1, path: '/x', source: null, preview: null, pages: null, origin: null, created_at: 0,
})

test('a private chat fetches its records, archive listings and exports naming the session; a durable chat does not', async () => {
  // The panel opening a record the store lacks (a lost `ArtifactShown`).
  useSessions.getState().setArtifacts(ROOT, [])
  await openShow(api as never, ROOT, 'p-note', 'a.md')
  expect(api.get_artifact).toHaveBeenLastCalledWith('p-note', ROOT)
  expect(useSessions.getState().sessions[ROOT]?.artifacts.get('p-note')?.id).toBe('id-p-note')
  useSessions.getState().setArtifacts('durable', [])
  await openShow(api as never, 'durable', 'd-note', 'd.md')
  expect(api.get_artifact).toHaveBeenLastCalledWith('d-note', undefined)

  // Save as… on the local daemon.
  await saveAs(api as never, record('p-note', ROOT), true)
  expect(api.export_artifact).toHaveBeenLastCalledWith('id-p-note', '/tmp/out/a.md', ROOT)
  await saveAs(api as never, record('d-note', 'durable'), true)
  expect(api.export_artifact).toHaveBeenLastCalledWith('id-d-note', '/tmp/out/a.md', undefined)

  // The archive view's listing: the component's own query, run from the cache it registered while rendering.
  for (const [call_id, session, owner] of [
    ['p-zip', ROOT, ROOT],
    ['d-zip', 'durable', undefined],
  ] as const) {
    const qc = new QueryClient()
    renderToStaticMarkup(createElement(QueryClientProvider, { client: qc }, createElement(TooltipProvider, null, createElement(ArchiveView, { a: record(call_id, session, 'archive') }))))
    await qc.getQueryCache().find({ queryKey: ['artifact-listing', call_id] })?.fetch()
    expect(api.get_artifact_listing).toHaveBeenLastCalledWith(call_id, owner)
  }
})
