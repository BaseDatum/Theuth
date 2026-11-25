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
  const row = at('<ComposerMeta\n')
  const bar = at('{/* 215 C2: row 2, the toolbar')
  expect(row).toBeGreaterThan(box)
  expect(row).toBeLessThan(at('<HeldStrip'))
  expect(row).toBeLessThan(at('<Textarea'))
  expect(src.slice(bar).includes('<ComposerMeta')).toBe(false)
  expect(at('<McpMenu')).toBeLessThan(at('<ApprovalShield'))
  expect(at('<ApprovalShield')).toBeGreaterThan(bar)
  expect(at('<ApprovalShield')).toBeLessThan(at('<div className="-ml-1 min-w-0 flex-1" />'))
  // No container-query shedding of the % or the cost.
  expect(readFileSync(new URL('./ComposerMeta.tsx', import.meta.url), 'utf8')).not.toMatch(/@max-|\/bar:hidden/)
})

test('the numbers exist for an existing session only (never the draft or a HUD composer); the row for the picker or the numbers, and its height comes out of the text\'s max', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  expect(src).toContain('const meta = useComposerMeta(!resolveSession && session ? session : null)')
  // The picker shows where the engine has the model methods, in any composer but a HUD one (no picker there).
  expect(src).toContain('const pickModel = useSupported(modelRequirements) && !resolveSession')
  expect(src).toContain('const topRow = pickModel || meta !== null')
  expect(src).toContain('{topRow && (')
  expect(src).toContain("const textMax = topRow ? (touch() ? 'max-h-[144px]' : 'max-h-[184px]') : touch() ? 'max-h-[160px]' : 'max-h-[200px]'")
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
  expect(row.querySelector('[data-meta-numbers]')!.className).not.toContain('ml-auto')
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

test('the bar fits its pane instead of pushing buttons out: tighter gaps and 32 px buttons; nothing is dropped', async () => {
  const { barFit, BAR_TIGHT_PX } = await import('./Composer')
  expect(barFit(false)).toBe('')
  expect(barFit(true).split(' ').sort()).toEqual(['[&_button.size-9]:size-8', 'gap-0.5'])
  expect(BAR_TIGHT_PX).toBe(300)
  expect(barFit(true)).not.toMatch(/:hidden/)
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  // Measured by a ResizeObserver, never a CSS container query: containment around the bar is what iOS WebKit has
  // left unpainted.
  expect(src).not.toMatch(/@container\/|@max-\[/)
  expect(src).toContain("className={cn('mt-1 flex items-center gap-1', barFit(barTight))}")
  expect(src).toContain('const barTight = useNarrow(box, BAR_TIGHT_PX)')
  expect(src).not.toContain('BAR_TINY_PX')
  // The shield is 28 px wide (the mock), never shed, and only pulses while an approval waits.
  const meta = readFileSync(new URL('./ComposerMeta.tsx', import.meta.url), 'utf8')
  expect(meta).toContain("'grid h-9 w-7 shrink-0 place-items-center")
  expect(meta.match(/animate-pulse/g)).toHaveLength(1)
  expect(meta).toContain("waiting: { icon: Shield, tone: 'text-danger motion-safe:animate-pulse'")
}, 30_000)

test('mic, send and talk keep exactly the conditions they had before the shield (86e625424~1)', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  // Talk: voice allowed, not dictating, no note held, an empty box.
  expect(src).toContain(") : canDictate && !dictating && stage === 'idle' && !text.trim() && (")
  // Mic: voice allowed, unless dictation is live (its stop lives in the voice bar).
  expect(src).toContain('{canDictate && !(dictating && dict.active) && (')
  expect(src).toContain('const canDictate = voice && !priv && !readOnly')
  // Send (or Steer + Stop while a turn runs) whenever no voice note holds the row.
  expect(src).toContain("{stage === 'idle' &&\n            (at !== 'idle' && session ? (")
  // (An attachment still uploading also holds Send: `waiting`.)
  expect(src).toContain('<SendButton disabled={!filled || busy || dictating || readOnly || !!starting} waiting={uploading}')
})

test('the box never clips its own content: no max height or overflow on the box, the text keeps a one-line floor', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const box = src.slice(src.indexOf('data-composer=""'), src.indexOf('onDragOver={(e) => e.preventDefault()}'))
  expect(box).not.toMatch(/overflow-hidden|max-h-/)
  // The max height is the text's alone (its own scroll area); the held strip and the top row add to the box.
  expect(src).toMatch(/const BOX =\n\s+'min-h-8 /)
  expect(src.match(/className=\{cn\(BOX, textMax\)\}/g)).toHaveLength(2)
})

/** The bottom bar's source: from its opening tag to the end of the box. */
function barSource(): string {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  return src.slice(src.indexOf('<div data-composer-bar=""'), src.indexOf('function DictatedText'))
}

test('the model picker is in the top row, before the numbers and the text, and not in the bottom bar', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const at = (s: string) => src.indexOf(s)
  const model = at('<ComposerModel session={session} own={ownModel} />')
  expect(src.match(/<ComposerModel /g)).toHaveLength(1)
  expect(model).toBeGreaterThan(at('<ComposerMeta\n'))
  expect(model).toBeLessThan(at('<Textarea'))
  expect(model).toBeLessThan(at('<div data-composer-bar=""'))
  // It goes during a voice note, as it did in the bar.
  expect(src).toContain("model={pickModel && stage === 'idle' ? <ComposerModel session={session} own={ownModel} /> : undefined}")
  // Opened from the top of the box, it lines up with the label's start.
  expect(readFileSync(new URL('./ModelMenu.tsx', import.meta.url), 'utf8')).toContain('<PopoverContent side="top" align="start"')
})

test('the bar holds only + · MCP · shield … spacer … talk · mic · send', () => {
  const bar = barSource()
  expect(bar).not.toMatch(/ComposerModel|ComposerMeta|data-tour="chat-model"/)
  const order = ['<Popover', '<McpMenu', '<ApprovalShield', '<div className="-ml-1 min-w-0 flex-1" />', 'aria-label="Talk"', '<MicButton', '<SendButton ']
  const idx = order.map((s) => bar.indexOf(s))
  expect(idx.every((i) => i >= 0)).toBe(true)
  expect([...idx].sort((a, b) => a - b)).toEqual(idx)
})

const pick = () => h('button', { type: 'button', 'data-tour': 'chat-model' }, 'Claude Opus 5.5 · medium')

test('the row: right-aligned, the picker then the numbers directly after it; the picker truncates first', () => {
  const box = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  // The picker's 32 px hover target hangs 8 px above the 16 px row: the box's top padding (8 px) keeps it inside the border.
  expect(box).toContain("topRow ? 'pt-2' : 'pt-2.5'")
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  act(() => root!.render(h(ComposerMeta, { session: 's1', meta: META, model: pick() })))
  const row = host.querySelector<HTMLElement>('[data-composer-meta]')!
  expect(row.className).toContain('h-4')
  const [first, second] = [...row.children] as HTMLElement[]
  expect(first!.dataset.tour).toBe('chat-model')
  expect(second!.hasAttribute('data-meta-numbers')).toBe(true)
  expect(second!.className).toContain('shrink-0')
  expect(second!.textContent).toBe('78%~$131')
  // The label shrinks (min-w-0) and keeps the row 16 px tall: its 32 px tap target hangs into negative margins.
  const quiet = readFileSync(new URL('./ModelMenu.tsx', import.meta.url), 'utf8')
  expect(quiet).toMatch(/const QUIET = '-mx-1 -my-2 h-auto min-w-0 max-w-full px-1 py-2 text-\[11px\] leading-4/)
})

test('the draft: the picker top-left alone, no numbers', () => {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  act(() => root!.render(h(ComposerMeta, { session: null, meta: null, model: pick() })))
  const row = host.querySelector<HTMLElement>('[data-composer-meta]')!
  expect(row.children).toHaveLength(1)
  expect((row.firstElementChild as HTMLElement).dataset.tour).toBe('chat-model')
  expect(row.querySelector('[data-meta-numbers]')).toBeNull()
  // The draft's composer is the same `<Composer>` (session null): one picker, in the same row.
  expect(readFileSync(new URL('./NewChatPane.tsx', import.meta.url), 'utf8')).toContain('<Composer')
})

test('recording with the picker: the row keeps its height, the picker goes, the numbers are invisible', () => {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  // Composer passes no `model` while a note records (`stage !== 'idle'`).
  act(() => root!.render(h(ComposerMeta, { session: null, meta: null, hidden: true })))
  const row = host.querySelector<HTMLElement>('[data-composer-meta]')!
  expect(row.className).toContain('h-4')
  expect(row.className).toContain('invisible')
  expect(row.querySelector('[data-tour="chat-model"]')).toBeNull()
})

test('219 §6.1: always-ask tools add a line that opens the Tools sheet; with approvals off the tooltip counts them', () => {
  const onAlwaysAsk = vi.fn()
  mount({ state: 'off', alwaysAsk: 2, onAlwaysAsk })
  expect(shield().getAttribute('title')).toBe('Approvals off · 2 tools always ask')
  click(shield())
  const line = document.querySelector<HTMLElement>('[data-level="always-ask"]')!
  expect(line.textContent).toBe('Always ask: 2 tools')
  click(line)
  expect(onAlwaysAsk).toHaveBeenCalled()
  act(() => root!.unmount())
  mount({ state: 'on', alwaysAsk: 0 })
  expect(shield().getAttribute('title')).toBe('Approvals on: asks before risky tools')
  click(shield())
  expect(document.querySelector('[data-level="always-ask"]')).toBeNull()
})
