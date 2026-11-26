import '../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TileHost } from '@/components/tiles/host'
import { SidebarProvider } from '@/components/ui/sidebar'
import { tileLeaf, usePanes } from '@/lib/panes'
import { tileIds, tileProps, tiling, untile } from '@/lib/panes.testing'
import { openPopout } from '@/lib/popout'
import { tileId } from '@/lib/tiles'
import type { Entry } from '@/store/sessions'
import { PtyViewCard, ptyCall } from './PtyViewCard'

const nav = vi.hoisted(() => ({ navigate: vi.fn() }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => nav.navigate }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<object>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))

type Tool = Extract<Entry, { kind: 'tool' }>
const tool = (over: Partial<Tool>): Tool => ({ id: 'e1', kind: 'tool', callId: 'c1', name: 'show_pty', args: { job: 'dev' }, pending: false, child: false, ...over })
const result = (output: string, isError = false) => ({ output, isError, files: [] })
const RECEIPT = 'pty view opened · Dev server · tile pty:s1/j3\nThe user can type into this terminal and the tile resizes it; you keep using screen()/send() as before.'
const JOB_RECEIPT = 'job view opened · cargo build · tile job:s1/j4\nThe tile shows its live log.'

beforeEach(() => {
  tiling(['s1'])
  nav.navigate.mockClear()
  vi.mocked(openPopout).mockClear()
})
afterEach(() => {
  untile()
  document.body.innerHTML = ''
})

test('the PTY receipt names the title and the job; a finished job says so', () => {
  expect(ptyCall(tool({ result: result(RECEIPT) }))).toEqual({ title: 'Dev server', job: { session: 's1', job: 'j3' }, pty: true, finished: false })
  const done = `${RECEIPT}\nThe job has finished; the tile shows its final screen.`
  expect(ptyCall(tool({ result: result(done) }))).toMatchObject({ finished: true })
  // A title with ` · ` in it still splits at the tile marker.
  expect(ptyCall(tool({ result: result('pty view opened · a · b · tile pty:s1/j12') })).title).toBe('a · b')
})

test('the background job receipt is the log form', () => {
  expect(ptyCall(tool({ result: result(JOB_RECEIPT) }))).toEqual({ title: 'cargo build', job: { session: 's1', job: 'j4' }, pty: false, finished: false })
  // The forms do not cross: a `tile pty:` marker after `job view opened` is neither.
  expect(ptyCall(tool({ result: result('job view opened · x · tile pty:s1/j4') })).job).toBeNull()
})

test('pending, failed or unparsable calls have no job; the arguments name them', () => {
  expect(ptyCall(tool({ pending: true }))).toEqual({ title: 'dev', job: null, pty: true, finished: false })
  expect(ptyCall(tool({ args: { job: 'j3', title: 'Build' } }))).toMatchObject({ title: 'Build', job: null })
  expect(ptyCall(tool({ args: { command: 'npm run dev' } })).title).toBe('npm run dev')
  expect(ptyCall(tool({ args: {} })).title).toBe('')
  expect(ptyCall(tool({ result: result('show_pty: no job named dev', true) })).job).toBeNull()
  expect(ptyCall(tool({ result: result('pty view opened · x · tile web:abc') })).job).toBeNull()
})

const mount = async (node: ReactNode, host: string | null) => {
  const el = document.body.appendChild(document.createElement('div'))
  const root = createRoot(el)
  await act(async () => root.render(h(SidebarProvider, null, h(TileHost.Provider, { value: host }, node))))
  return { el, root }
}
const openButton = (el: HTMLElement) => [...el.querySelectorAll('button')].find((b) => /^(Open|Reopen|Focus)$/.test(b.textContent ?? ''))!

test('Open puts the terminal tile beside the chat where the layout tiles', async () => {
  const { el, root } = await mount(h(PtyViewCard, { tool: tool({ result: result(RECEIPT) }) }), 's1')
  await act(async () => openButton(el).click())
  const id = tileId('job-terminal', 's1', 's1/j3')
  expect(usePanes.getState().leaves).toEqual(['s1', tileLeaf(id)])
  expect(tileProps(id)).toEqual({ session: 's1', job: 'j3', label: 'Dev server' })
  await act(async () => root.unmount())
})

test('Open on a background job puts its log tile beside the chat, with no pop out', async () => {
  const { el, root } = await mount(h(PtyViewCard, { tool: tool({ result: result(JOB_RECEIPT) }) }), 's1')
  expect(el.querySelector('button[aria-label="Open in new window"]')).toBeNull()
  await act(async () => openButton(el).click())
  const id = tileId('job-log', 's1', 's1/j4')
  expect(usePanes.getState().leaves).toEqual(['s1', tileLeaf(id)])
  expect(tileProps(id)).toEqual({ session: 's1', job: 'j4', label: 'cargo build' })
  await act(async () => root.unmount())
})

test('with no tile host Open pops the terminal out, never tiling', async () => {
  const { el, root } = await mount(h(PtyViewCard, { tool: tool({ result: result(RECEIPT) }) }), null)
  await act(async () => openButton(el).click())
  expect(openPopout).toHaveBeenCalledWith({ kind: 'pty', session: 's1', job: 'j3' })
  expect(tileIds()).toEqual([])
  await act(async () => root.unmount())
})
