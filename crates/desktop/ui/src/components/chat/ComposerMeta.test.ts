import '../settings/test-dom'
import { readFileSync } from 'node:fs'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { approvalsChoice, setApprovalsLevel } from './approvals-level'
import { ApprovalShieldView, costBallpark, shieldState } from './ComposerMeta'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn() }) }))

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

test('the composer bar: shield right of MCP on the left, the strip right of the model picker; cost sheds before context', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const at = (s: string) => src.indexOf(s)
  expect(at('<McpMenu')).toBeLessThan(at('<ApprovalShield'))
  expect(at('<ApprovalShield')).toBeLessThan(at('<div className="-ml-1 min-w-0 flex-1" />'))
  expect(at('<ComposerModel')).toBeLessThan(at('<ComposerMeta'))
  const meta = readFileSync(new URL('./ComposerMeta.tsx', import.meta.url), 'utf8')
  const shed = (k: string) => Number(meta.match(new RegExp(`data-meta="${k}"[\\s\\S]*?@max-\\[(\\d+)px\\]/bar:hidden`))?.[1])
  expect(shed('cost')).toBeGreaterThan(shed('context'))
  // The shield has no container-query class: it never sheds.
  expect(meta.slice(meta.indexOf('data-shield'), meta.indexOf('</PopoverTrigger>'))).not.toContain('@max-')
})
