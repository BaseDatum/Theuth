import type { Api } from '@/rpc/bindings/client'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import * as Layout from '@/lib/panes.testing'
import * as Tiles from '@/lib/tiles'
import type { Artifact } from "@/rpc/bindings/method.artifact.output"
import { type Entry, createSessions, useSessions } from '@/store/sessions'
import { ArtifactCard, openShow, relistArtifacts } from './ArtifactCard'

// The receipt card renders outside a QueryClient and before any connection; the inbox mark and the api are stubbed.
const api = vi.hoisted(() => ({ get_artifact: vi.fn<(id: string) => Promise<Artifact>>() }))
const toast = vi.hoisted(() => ({ error: vi.fn(), success: vi.fn() }))
vi.mock('sonner', () => ({ toast }))
vi.mock('@/components/hud/queries', () => ({ useMarkShown: () => {} }))
// The layout tiles only where a test says so (`tileable` reads the DOM: none here).
const tiling = vi.hoisted(() => ({ on: false }))
vi.mock('@/lib/tiles', async (original) => ({ ...(await original<typeof Tiles>()), tileable: () => tiling.on }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

afterEach(() => vi.clearAllMocks())

type Tool = Extract<Entry, { kind: 'tool' }>
const S = 'sess'
const tool = (extra: Partial<Tool> = {}): Tool => ({
  id: 'e1',
  kind: 'tool',
  callId: 'c1',
  name: 'show',
  args: { path: '/tmp/137-endless-morning-report.md' },
  pending: false,
  child: false,
  result: { output: 'shown 137-endless-morning-report.md', isError: false, files: [] },
  ...extra,
})
const artifact: Artifact = {
  id: 'abc',
  session: S,
  call_id: 'c1',
  name: '137-endless-morning-report.md',
  title: 'Morning report',
  kind: 'markdown',
  mime: 'text/markdown',
  bytes: 5000,
  version: 1,
  path: '/data/artifacts/abc.md',
  source: null,
  preview: null,
  pages: null,
  origin: null,
  created_at: 0,
}
const fresh = () => {
  const st = useSessions.getState()
  st.setArtifacts(S, [])
  st.setArtifactPanel(S, null)
  return st
}

test('receipt: a finished show without its record is a button; a failed one is not', () => {
  fresh()
  const ok = renderToStaticMarkup(createElement(ArtifactCard, { tool: tool(), session: S }))
  expect(ok).toContain('role="button"')
  expect(ok).toContain('shown 137-endless-morning-report.md')
  const failed = renderToStaticMarkup(
    createElement(ArtifactCard, { tool: tool({ result: { output: 'no such file', isError: true, files: [] } }), session: S }),
  )
  expect(failed).not.toContain('role="button"')
  // Still compiling: nothing to open yet.
  const compiling = renderToStaticMarkup(createElement(ArtifactCard, { tool: tool({ result: undefined }), session: S }))
  expect(compiling).not.toContain('role="button"')
})

test('openShow: no record → get_artifact, applied as ArtifactShown, panel opened; a record present skips the fetch', async () => {
  fresh()
  api.get_artifact.mockResolvedValueOnce(artifact)
  await openShow(api, S, 'c1', artifact.name)
  // A durable chat names no session (51 §4.2: only a private tree's owner does).
  expect(api.get_artifact).toHaveBeenCalledWith('c1', undefined)
  const s = useSessions.getState().sessions[S]
  expect(s?.artifacts.get('c1')).toEqual(artifact)
  expect(s?.artifactPanel).toBe('c1')

  useSessions.getState().setArtifactPanel(S, null)
  await openShow(api, S, 'c1', artifact.name)
  expect(api.get_artifact).toHaveBeenCalledTimes(1)
  expect(useSessions.getState().sessions[S]?.artifactPanel).toBe('c1')
})

test('openShow: a failed get_artifact toasts "could not open <name>" and leaves the card and panel alone', async () => {
  fresh()
  api.get_artifact.mockRejectedValueOnce(new Error('not connected'))
  await openShow(api, S, 'c1', artifact.name)
  expect(toast.error).toHaveBeenCalledWith('could not open 137-endless-morning-report.md')
  const s = useSessions.getState().sessions[S]
  expect(s?.artifacts.has('c1')).toBe(false)
  expect(s?.artifactPanel).toBeNull()
})

test('openShow where the layout tiles: one artifact tile per chat, retargeted on the next open; the panel stays shut', async () => {
  fresh()
  tiling.on = true
  Layout.tiling([S])
  try {
    useSessions.getState().applyEvents([{ type: 'ArtifactShown', session: S, call_id: 'c1', artifact }])
    await openShow(api, S, 'c1', artifact.name)
    expect(Tiles.openTileOf('artifact', S)?.props).toEqual({ callId: 'c1' })
    useSessions.getState().applyEvents([{ type: 'ArtifactShown', session: S, call_id: 'c2', artifact: { ...artifact, call_id: 'c2' } }])
    await openShow(api, S, 'c2', artifact.name)
    expect(Tiles.openTileOf('artifact', S)?.props).toEqual({ callId: 'c2' })
    expect(Tiles.tiles().filter((t) => t.kind === 'artifact')).toHaveLength(1)
    expect(useSessions.getState().sessions[S]?.artifactPanel ?? null).toBeNull()
  } finally {
    tiling.on = false
    Layout.untile()
  }
})

test('relistArtifacts installs the list; a rejected list leaves the existing map intact, never []', async () => {
  const store = createSessions()
  const old = { ...artifact, call_id: 'old' }
  store.getState().setArtifacts(S, [old])
  const list_artifacts = vi.fn<Api['list_artifacts']>()

  list_artifacts.mockResolvedValueOnce([old, artifact])
  await relistArtifacts({ list_artifacts }, store, S)
  expect(list_artifacts).toHaveBeenCalledWith(S, null)
  expect([...store.getState().sessions[S]!.artifacts.keys()]).toEqual(['old', 'c1'])

  list_artifacts.mockRejectedValueOnce(new Error('not connected'))
  await expect(relistArtifacts({ list_artifacts }, store, S)).resolves.toBeUndefined()
  expect([...store.getState().sessions[S]!.artifacts.keys()]).toEqual(['old', 'c1'])
})
