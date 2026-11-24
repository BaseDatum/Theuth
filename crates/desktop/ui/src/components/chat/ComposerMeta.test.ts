import '../settings/test-dom'
import { readFileSync } from 'node:fs'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { approvalsChoice, setApprovalsLevel } from './approvals-level'
import { ApprovalShieldView, ComposerMeta, costBallpark, shieldState } from './ComposerMeta'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn() }) }))

test('cost: whole units from 1 up, cents only below 1', () => {
  expect(costBallpark(1.62)).toBe('~$2')
  expect(costBallpark(44.09)).toBe('~$44')
  expect(costBallpark(1)).toBe('~$1')
  expect(costBallpark(0.38)).toBe('~$0.38')
  expect(costBallpark(0.999)).toBe('~$1.00')
  expect(costBallpark(0)).toBe('~$0.00')
  expect(costBallpark(131.5, '€')).toBe('~€132')
})

test('shield state: a waiting approval outranks the level; else off or on', () => {
  expect(shieldState(0, false)).toBe('on')
  expect(shieldState(0, true)).toBe('off')
  expect(shieldState(1, true)).toBe('waiting')
  expect(shieldState(2, false)).toBe('waiting')
})

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null
afterEach(() => {
  act(() => root?.unmount())
  host?.remove()
  root = null
})

function mount(props: Partial<Parameters<typeof ApprovalShieldView>[0]> = {}) {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  const onPick = vi.fn()
  const onJump = vi.fn()
  const choice = approvalsChoice({ approvals: { global: true, session: true, pin: null } })
  act(() => root!.render(h(ApprovalShieldView, { state: 'on', choice, waiting: 0, onPick, onJump, ...props })))
  return { onPick, onJump }
}

const shield = () => document.querySelector<HTMLButtonElement>('[data-shield]')!

test('the three shield states: muted shield-check, amber struck shield, filled red pulsing', () => {
  mount({ state: 'on' })
  expect(shield().dataset.shield).toBe('on')
  expect(shield().className).toContain('text-fg-tertiary')
  expect(shield().querySelector('svg')?.getAttribute('fill')).toBe('none')
  act(() => root!.unmount())
  mount({ state: 'off' })
  expect(shield().className).toContain('text-warning')
  expect(shield().getAttribute('aria-label')).toMatch(/off/)
  act(() => root!.unmount())
  mount({ state: 'waiting', waiting: 1 })
  expect(shield().className).toContain('text-danger')
  expect(shield().className).toContain('animate-pulse')
  expect(shield().querySelector('svg')?.getAttribute('fill')).toBe('currentColor')
})

const click = (el: Element) => act(() => void el.dispatchEvent(new MouseEvent('click', { bubbles: true })))

test('clicking the shield opens the popover with the levels the engine has; picking one hands its value over', () => {
  const { onPick } = mount()
  expect(document.querySelector('[data-level]')).toBeNull()
  click(shield())
  const levels = [...document.querySelectorAll<HTMLElement>('[role="radio"]')]
  expect(levels.map((l) => l.dataset.level)).toEqual(['on', 'off', 'default'])
  expect(levels.map((l) => l.textContent)).toEqual(['Ask before risky tools', 'Never ask (off)', 'Machine default (on)'])
  expect(levels.find((l) => l.getAttribute('aria-checked') === 'true')?.dataset.level).toBe('default')
  expect(document.querySelector('[data-level="jump"]')).toBeNull()
  click(levels[1]!)
  expect(onPick).toHaveBeenCalledWith('off')
})

test('with an approval waiting the popover leads with the jump row', () => {
  const { onJump } = mount({ state: 'waiting', waiting: 1 })
  click(shield())
  const rows = [...document.querySelectorAll<HTMLElement>('[data-level]')]
  expect(rows[0]?.dataset.level).toBe('jump')
  expect(rows[0]?.textContent).toBe('1 waiting: jump to it')
  click(rows[0]!)
  expect(onJump).toHaveBeenCalled()
})

test('a level goes out through set_approvals exactly as the ⋯ menu sends it, and both call the one setter', async () => {
  const set_approvals = vi.fn(async () => ({ global: true, session: true }))
  const api = { set_approvals } as unknown as Api
  await setApprovalsLevel(api, 's1', 'on', false)
  await setApprovalsLevel(api, 's1', 'off', false)
  await setApprovalsLevel(api, 's1', 'default', false)
  expect(set_approvals.mock.calls).toEqual([['s1', true], ['s1', false], ['s1', false, true]])
  for (const f of ['./ComposerMeta.tsx', './SessionMenu.tsx']) expect(readFileSync(new URL(f, import.meta.url), 'utf8')).toContain('setApprovalsLevel(api, session,')
})

test('the meta is a row at the top of the box, before the text in DOM order, never in the bottom bar; the shield stays in the bar after MCP', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const at = (s: string) => src.indexOf(s)
  const box = at('data-composer=""')
  const row = at('<ComposerMeta session={session} meta={meta}')
  const bar = at('{/* 215 C2: row 2, the toolbar')
  expect(row).toBeGreaterThan(box)
  expect(row).toBeLessThan(at('<HeldStrip'))
  expect(row).toBeLessThan(at('<Textarea'))
  expect(src.slice(bar).includes('<ComposerMeta')).toBe(false)
  expect(at('<McpMenu')).toBeLessThan(at('<ApprovalShield'))
  expect(at('<ApprovalShield')).toBeGreaterThan(bar)
  expect(at('<ApprovalShield')).toBeLessThan(at('<div className="-ml-1 min-w-0 flex-1" />'))
  // No container-query shedding anywhere in the strip or the bar any more.
  expect(readFileSync(new URL('./ComposerMeta.tsx', import.meta.url), 'utf8')).not.toMatch(/@max-|\/bar:hidden/)
  expect(src).not.toContain('@container/bar')
})

test('the row exists for an existing session only (never the draft or a HUD composer), and its height comes out of the text\'s max', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  expect(src).toContain('const meta = useComposerMeta(!resolveSession && session ? session : null)')
  expect(src).toContain("const textMax = meta ? (touch() ? 'max-h-[144px]' : 'max-h-[184px]') : touch() ? 'max-h-[160px]' : 'max-h-[200px]'")
  expect(src.match(/className=\{cn\(BOX, textMax\)\}/g)).toHaveLength(2)
  // A voice note keeps the row (same height, empty): no jump.
  expect(src).toContain('hidden={stage !== \'idle\'}')
})

const META = { pct: 78, estimated: false, ctxTitle: '780k / 1000k tokens\nCache 75%', cost: { short: '~$131', exact: '$131.40', title: '$131.40' } }

test('the row: both numbers always shown, right-aligned 16 px, the % banded amber from 75, red from 90; the cost opens the breakdown', () => {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  act(() => root!.render(h(ComposerMeta, { session: 's1', meta: META })))
  const row = host.querySelector<HTMLElement>('[data-composer-meta]')!
  expect(row.className).toContain('h-4')
  expect(row.className).toContain('justify-end')
  expect(row.className).not.toContain('invisible')
  const pct = row.querySelector<HTMLElement>('[data-meta="context"]')!
  expect(pct.textContent).toBe('78%')
  expect(pct.className).toContain('text-warning')
  expect(pct.title).toBe('780k / 1000k tokens\nCache 75%')
  const cost = row.querySelector<HTMLElement>('[data-meta="cost"]')!
  expect(cost.textContent).toBe('~$131')
  const opened = vi.fn()
  window.addEventListener('theuth:open-cost', opened)
  click(cost)
  window.removeEventListener('theuth:open-cost', opened)
  expect((opened.mock.calls[0]?.[0] as CustomEvent).detail).toEqual({ session: 's1' })
  act(() => root!.render(h(ComposerMeta, { session: 's1', meta: { ...META, pct: 91 } })))
  expect(row.querySelector<HTMLElement>('[data-meta="context"]')!.className).toContain('text-danger')
})

test('recording: the row keeps its height but shows nothing and takes no focus', () => {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  act(() => root!.render(h(ComposerMeta, { session: 's1', meta: META, hidden: true })))
  const row = host.querySelector<HTMLElement>('[data-composer-meta]')!
  expect(row.className).toContain('invisible')
  expect(row.className).toContain('h-4')
  expect(row.getAttribute('aria-hidden')).toBe('true')
  expect(row.querySelector<HTMLElement>('[data-meta="cost"]')!.tabIndex).toBe(-1)
})
