import { isRedirect } from '@tanstack/react-router'
import { afterEach, expect, test, vi } from 'vitest'
// The registry imports every tab component; some stores read `localStorage` at import; no DOM in the runner.
vi.hoisted(() => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
})
import { Route as ToolsRoute } from '@/routes/tools'
import { CATEGORIES } from './registry'
import { routeTarget } from './route-target'

const tabs = CATEGORIES.flatMap((category) => [...category.tabs])
const original = new Map(tabs.map((tab) => [tab, tab.visible]))
/** Every tab shown but `hidden`: a real `visible` (`hasLocal()`) reads the container, which node has not. */
const hide = (...hidden: string[]) => {
  for (const tab of tabs) tab.visible = () => !hidden.includes(tab.id)
}
afterEach(() => {
  for (const [tab, visible] of original) tab.visible = visible
})

test('a shown tab renders, whatever the search', () => {
  hide()
  expect(routeTarget('contacts', {})).toEqual({ kind: 'render' })
  expect(routeTarget('memory', { focus: 'spend' })).toEqual({ kind: 'render' })
  expect(routeTarget('messaging', { session: 's1' })).toEqual({ kind: 'render' })
  expect(routeTarget('tools', { session: 's1' })).toEqual({ kind: 'render' })
})

test('1: tabs that became pages leave Settings, keeping a legacy session', () => {
  hide()
  expect(routeTarget('skills', { session: 's1', focus: 'x' })).toEqual({ kind: 'page', to: '/skills', search: { session: 's1' } })
  expect(routeTarget('secrets', {})).toEqual({ kind: 'page', to: '/secrets', search: {} })
  expect(routeTarget('schedules', {})).toEqual({ kind: 'page', to: '/routines', search: {} })
})

/** Where `/tools?<raw>` sends the router: the redirect its `beforeLoad` throws for the validated search. */
function toolsLanding(raw: Record<string, unknown>) {
  const search = (ToolsRoute.options.validateSearch as (s: Record<string, unknown>) => { session?: string })(raw)
  try {
    ToolsRoute.options.beforeLoad!({ search } as never)
  } catch (e) {
    if (isRedirect(e)) return e.options
    throw e
  }
  throw new Error('/tools did not redirect')
}

test('Sky 2026-11-11: /tools?session=x lands on Settings › Tools › MCP with session=x, which renders there', () => {
  hide()
  const landing = toolsLanding({ session: 'x', focus: 'y' })
  expect(landing).toMatchObject({ to: '/settings/$tab', params: { tab: 'tools' }, replace: true })
  expect(landing.search).toEqual({ session: 'x' })
  expect(routeTarget('tools', { session: 'x' })).toEqual({ kind: 'render' })
  expect(toolsLanding({}).search).toEqual({})
})

test('2: an old communication link that names a chat opens that chat’s Messaging sheet, with its peer request', () => {
  hide()
  expect(routeTarget('communication', { session: 's1' })).toEqual({ kind: 'chat', session: 's1', search: { messaging: '1' } })
  expect(routeTarget('communication', { session: 's1', peer: 'm9' })).toEqual({ kind: 'chat', session: 's1', search: { messaging: '1', peer: 'm9' } })
})

test('3: old ids land on the tab that holds their settings, keeping focus; without a chat, communication is Messaging', () => {
  hide()
  expect(routeTarget('communication', { peer: 'm9' })).toEqual({ kind: 'tab', tab: 'messaging', search: {} })
  expect(routeTarget('general', {})).toEqual({ kind: 'tab', tab: 'appearance', search: {} })
  expect(routeTarget('mesh', {})).toEqual({ kind: 'tab', tab: 'machine', search: {} })
  expect(routeTarget('network', { focus: 'f' })).toEqual({ kind: 'tab', tab: 'machine', search: { focus: 'f' } })
  expect(routeTarget('labs', {})).toEqual({ kind: 'tab', tab: 'decision', search: {} })
})

test('3 + S4: an old id whose new tab is hidden lands on the next shown tab of that category', () => {
  hide('machine')
  expect(routeTarget('network', {})).toEqual({ kind: 'tab', tab: 'machines', search: {} })
})

test('4: an unknown id lands on Appearance; prototype keys are unknown too', () => {
  hide()
  expect(routeTarget('nonsense', {})).toEqual({ kind: 'tab', tab: 'appearance', search: {} })
  expect(routeTarget('constructor', {})).toEqual({ kind: 'tab', tab: 'appearance', search: {} })
  expect(routeTarget('__proto__', { focus: 'f' })).toEqual({ kind: 'tab', tab: 'appearance', search: { focus: 'f' } })
})

test('S4: a hidden tab lands on its category’s first shown tab, else Appearance', () => {
  hide('vms', 'machine')
  expect(routeTarget('vms', {})).toEqual({ kind: 'tab', tab: 'machines', search: {} })
  hide('signal', 'slack', 'telegram')
  expect(routeTarget('telegram', {})).toEqual({ kind: 'tab', tab: 'appearance', search: {} })
})
