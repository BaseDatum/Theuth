import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { toast } from 'sonner'
import { expect, test, vi } from 'vitest'
import type { AppDoc } from "@/rpc/bindings/method.app.output"
import { useSessions } from '@/store/sessions'
import { FenceContext } from './JsonRenderFence'
import { INERT, OpenUIFence, appName, openAsApp, staticProvider } from './OpenUIFence'

// `useDark` reads `document`, `useApi` the connection store, `toast` the DOM: none exist in a node test.
vi.mock('@/components/apps/theme', () => ({ useDark: () => false }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }))

const SRC = 'root = Stack([t])\nt = TextContent("hi")'
const fence = (code: string, isIncomplete: boolean) => createElement(OpenUIFence, { code, isIncomplete, language: 'openui' })

test('a closed two-statement program renders its text merged into the flow: no card chrome, no title bar, hover toolbar', () => {
  const html = renderToStaticMarkup(fence(SRC, false))
  expect(html).toContain('data-streamdown="openui"')
  expect(html).toContain('hi')
  expect(html).not.toContain('rounded-xl border')
  expect(html).not.toContain('>openui<')
  expect(html).not.toContain('Open as app')
  expect(html).toMatch(/group-hover:opacity-100[^>]*>.*>Show source<\/button>/)
  expect(html).not.toContain('<pre')
})

test('inside a live transcript the frame offers Open as app; disabled while the fence streams', () => {
  const at = (isIncomplete: boolean) =>
    renderToStaticMarkup(createElement(FenceContext, { value: { running: isIncomplete, hasLaterAssistant: false, session: 's1' } }, fence(SRC, isIncomplete)))
  expect(at(false)).toContain('>Open as app</button>')
  expect(at(false)).not.toMatch(/<button[^>]*\sdisabled=""/)
  expect(at(true)).toMatch(/<button[^>]*\sdisabled=""[^>]*>Open as app/)
})

test('a closed program that does not parse is the quiet fallback, not a red card', () => {
  const html = renderToStaticMarkup(fence('root = Stack([', false))
  expect(html).toContain('diagram omitted')
  expect(html).not.toContain('role="alert"')
})

test('the static provider rejects every call with the open-as-app hint', async () => {
  await expect(staticProvider('tool', {}, { statement_id: 'q', trigger: 'load' })).rejects.toThrow(INERT)
})

test('the app name comes from the first ## heading, else reply-<id>', () => {
  expect(appName('root = Stack([h])\nh = TextContent("## Weekly Report: Q3!")')).toBe('weekly-report-q3')
  expect(appName(SRC)).toMatch(/^reply-[0-9a-f]{8}$/)
})

test('Open as app publishes under the derived name and opens the panel on the new record', async () => {
  const artifact = { call_id: 'rec9', name: 'weekly-report', session: 's1' } as AppDoc['artifact']
  const publish_app = vi.fn(async () => ({ artifact }) as AppDoc)
  await openAsApp({ publish_app }, 's1', 'root = TextContent("## Weekly report")')
  expect(publish_app).toHaveBeenCalledWith('s1', 'weekly-report', 'root = TextContent("## Weekly report")')
  const s = useSessions.getState().sessions.s1
  expect(s?.artifactPanel).toBe('rec9')
  expect(s?.artifacts.get('rec9')).toMatchObject({ call_id: 'rec9', name: 'weekly-report' })
})

test('a publish failure toasts the message and opens nothing', async () => {
  await openAsApp({ publish_app: vi.fn(async () => Promise.reject(new Error('statement q: unknown tool'))) }, 's2', SRC)
  expect(toast.error).toHaveBeenCalledWith('statement q: unknown tool')
  expect(useSessions.getState().sessions.s2?.artifactPanel ?? null).toBeNull()
})
