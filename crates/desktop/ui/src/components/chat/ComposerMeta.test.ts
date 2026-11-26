import '../settings/test-dom'
import { readFileSync } from 'node:fs'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { approvalsChoice, setApprovalsLevel } from './approvals-level'
import { ApprovalShieldView, ComposerMeta, ContextButtonView, costBallpark, shieldState } from './ComposerMeta'

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

test('the three shield states: muted shield-check, carnelian struck shield, filled carnelian pulsing', () => {
  mount({ state: 'on' })
  expect(shield().dataset.shield).toBe('on')
  expect(shield().className).toContain('text-fg-tertiary')
  expect(shield().querySelector('svg')?.getAttribute('fill')).toBe('none')
  act(() => root!.unmount())
  mount({ state: 'off' })
  expect(shield().className).toContain('text-danger')
  expect(shield().className).not.toContain('animate-pulse')
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
  expect(at('<ApprovalShield')).toBeLessThan(at('data-model-slot=""'))
  // No container-query shedding of the % or the cost.
  expect(readFileSync(new URL('./ComposerMeta.tsx', import.meta.url), 'utf8')).not.toMatch(/@max-|\/bar:hidden/)
})

test('the row exists for an existing session with a reading only (never the draft or a HUD composer), and its height comes out of the text\'s max', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  expect(src).toContain('const meta = useComposerMeta(!resolveSession && session ? session : null)')
  expect(src).toContain('{meta && session && <ComposerMeta session={session} meta={meta} hidden={stage !== \'idle\'} />}')
  // The box's top padding: 4 px over the row, 10 px with none (no room for a picker hover any more).
  expect(src).toContain("shadow-sm', meta ? 'pt-1' : 'pt-2.5')}")
  expect(src).not.toMatch(/topRow|pickModel/)
  expect(src).toContain("const textMax = meta ? (touch() ? 'max-h-[144px]' : 'max-h-[184px]') : touch() ? 'max-h-[160px]' : 'max-h-[200px]'")
  expect(src.match(/className=\{cn\(BOX, textMax\)\}/g)).toHaveLength(2)
  // A voice note keeps the row (same height, empty): no jump.
  expect(src).toContain('hidden={stage !== \'idle\'}')
})

const META = { cost: { short: '~$131', exact: '$131.40', title: '$131.40' } }

test('the row: only the cost, right-aligned; no context percent (the context button has it); the cost opens the breakdown', () => {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  act(() => root!.render(h(ComposerMeta, { session: 's1', meta: META })))
  const row = host.querySelector<HTMLElement>('[data-composer-meta]')!
  expect(row.className).toContain('h-3')
  expect(row.className).toContain('justify-end')
  expect([...row.children].map((c) => (c as HTMLElement).dataset.meta)).toEqual(['cost'])
  expect(row.querySelector('[data-meta="context"]')).toBeNull()
  expect(row.textContent).not.toContain('%')
  expect(row.querySelector('[data-tour="chat-model"]')).toBeNull()
  expect(row.className).not.toContain('invisible')
  const cost = row.querySelector<HTMLElement>('[data-meta="cost"]')!
  expect(cost.textContent).toBe('~$131')
  expect(cost.title).toBe('$131.40')
  const opened = vi.fn()
  window.addEventListener('theuth:open-cost', opened)
  click(cost)
  window.removeEventListener('theuth:open-cost', opened)
  expect((opened.mock.calls[0]?.[0] as CustomEvent).detail).toEqual({ session: 's1' })
})

test('recording: the row keeps its height but shows nothing and takes no focus', () => {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  act(() => root!.render(h(ComposerMeta, { session: 's1', meta: META, hidden: true })))
  const row = host.querySelector<HTMLElement>('[data-composer-meta]')!
  expect(row.className).toContain('invisible')
  expect(row.className).toContain('h-3')
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
  // The shield shares the row's icon box (shrunk with the rest by `barFit`), is never shed, and only pulses while an approval waits.
  const meta = readFileSync(new URL('./ComposerMeta.tsx', import.meta.url), 'utf8')
  expect(meta).toContain("className={cn(COMPOSER_ICON_BOX, 'grid place-items-center outline-none")
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

test('the model picker is on the bar, right before talk · mic · send, and not in the top row', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const bar = barSource()
  expect(src.match(/<ComposerModel /g)).toHaveLength(1)
  expect(src.slice(0, src.indexOf('<div data-composer-bar=""')).includes('<ComposerModel ')).toBe(false)
  expect(readFileSync(new URL('./ComposerMeta.tsx', import.meta.url), 'utf8')).not.toMatch(/ComposerModel|chat-model|model\?: ReactNode/)
  // + · MCP · shield … [slot: spacer + model] · talk · mic · send
  const order = ['<Popover', '<McpMenu', '<ApprovalShield', 'data-model-slot=""', '<ComposerModel session={session} own={ownModel} room={modelRoom} />', 'aria-label="Talk"', '<MicButton', '<SendButton ']
  const idx = order.map((s) => bar.indexOf(s))
  expect(idx.every((i) => i >= 0)).toBe(true)
  expect([...idx].sort((a, b) => a - b)).toEqual(idx)
  // The slot is the bar's one flex-1 item from a 0 basis, so its measured width is the room the buttons leave.
  expect(bar).toContain('<div ref={modelSlot} data-model-slot="" className="-ml-1 flex min-w-0 flex-1 justify-end">')
  expect(bar.match(/className="[^"]*\bflex-1\b/g)).toHaveLength(1)
  expect(src).toContain('const modelRoom = useWidth(modelSlot)')
  // Hidden during a voice note and in a HUD composer, as before.
  expect(bar).toContain("{!resolveSession && stage === 'idle' && <ComposerModel")
  // The menu still opens upward.
  expect(readFileSync(new URL('./ModelMenu.tsx', import.meta.url), 'utf8')).toContain('<PopoverContent side="top" align="end"')
})

test('no button is ever hidden or shrunk for room: only the label shortens', () => {
  const bar = barSource()
  expect(bar).not.toMatch(/:hidden|max-sm:hidden|@container|@max-/)
  const menu = readFileSync(new URL('./ModelMenu.tsx', import.meta.url), 'utf8')
  expect(menu).toContain("const QUIET = 'h-8 max-w-full min-w-0'")
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

test('context button: the ring fills with the used share and warns from 80 %, danger from 95 %; compact by style, shake, and a two-tap clear', () => {
  vi.useFakeTimers()
  try {
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
    const onAction = vi.fn()
    const show = (pct: number | null) => act(() => root!.render(h(ContextButtonView, { reading: pct === null ? null : { used: 128_000, limit: 200_000, pct, cache: null, estimated: false }, onAction })))
    const button = () => document.querySelector<HTMLButtonElement>('[data-context-button]')!
    const tone = () => button().className.split(' ').filter((c) => /^text-(fg-tertiary|warning|danger)$/.test(c))
    show(null)
    expect(tone()).toEqual(['text-fg-tertiary'])
    expect(button().querySelector('[data-ring-used]')).toBeNull()
    for (const [pct, want] of [[64, 'text-fg-tertiary'], [79, 'text-fg-tertiary'], [80, 'text-warning'], [94, 'text-warning'], [95, 'text-danger']] as const) {
      show(pct)
      expect(tone(), `${pct}%`).toEqual([want])
    }
    show(64)
    const used = button().querySelector('[data-ring-used]')!.getAttribute('stroke-dasharray')!.split(' ').map(Number)
    expect(used[0]! / used[1]!).toBeCloseTo(0.64)
    click(button())
    expect(document.body.textContent).toContain('128k of 200k · 64%')

    // Compact: one icon button per style, each labelled, ≥ 36 px, running exactly that style.
    const styles = [...document.querySelectorAll<HTMLButtonElement>('[data-compact-style]')]
    expect(styles.map((b) => b.dataset.compactStyle)).toEqual(['snap', 'cut', 'summary'])
    expect(styles.map((b) => b.getAttribute('aria-label'))).toEqual([
      'Compact: snap — keep older turns as images',
      'Compact: cut — drop older turns, keep the recent ones',
      'Compact: summary — fold older turns into a summary',
    ])
    expect(styles.every((b) => b.title === b.getAttribute('aria-label') && b.className.split(' ').includes('size-9'))).toBe(true)
    const caption = () => document.querySelector('[data-compact-caption]')!.textContent
    // The popover focuses its first control on open, so the caption starts on snap; hover moves it, leaving restores the static line.
    expect(caption()).toBe('Snap — keep older turns as images')
    act(() => void styles[2]!.dispatchEvent(new MouseEvent('mouseover', { bubbles: true })))
    expect(caption()).toBe('Summary — fold older turns into a summary')
    act(() => void styles[2]!.dispatchEvent(new MouseEvent('mouseout', { bubbles: true, relatedTarget: document.body })))
    act(() => styles[0]!.blur())
    expect(caption()).toBe('Fold older turns: snap, cut or summary')
    act(() => styles[1]!.focus())
    expect(caption()).toBe('Cut — drop older turns, keep the recent ones')
    act(() => styles[1]!.blur())
    expect(caption()).toBe('Fold older turns: snap, cut or summary')
    for (const b of styles) click(b)
    expect(onAction.mock.calls).toEqual([[{ compact: 'snap' }], [{ compact: 'cut' }], [{ compact: 'summary' }]])
    onAction.mockClear()

    const rows = [...document.querySelectorAll<HTMLElement>('[data-context-action]')]
    expect(rows.map((r) => r.dataset.contextAction)).toEqual(['/shake', '/clear'])
    expect(rows.every((r) => r.className.split(' ').includes('min-h-9'))).toBe(true)
    const clear = rows[1]!
    const armed = () => clear.hasAttribute('data-confirm')

    // Clear: the first tap only arms it; the second within 4 s clears.
    click(clear)
    expect(onAction).not.toHaveBeenCalled()
    expect(armed()).toBe(true)
    expect(clear.className.split(' ')).toContain('bg-danger')
    expect(clear.textContent).toContain('Tap again to clear')
    expect(clear.textContent).toContain('This drops the whole conversation')
    act(() => void vi.advanceTimersByTime(3900))
    click(clear)
    expect(onAction.mock.calls).toEqual([['/clear']])
    expect(armed()).toBe(false)
    onAction.mockClear()

    // It disarms by itself after 4 s: the next tap arms again rather than clearing.
    click(clear)
    act(() => void vi.advanceTimersByTime(4000))
    expect(armed()).toBe(false)
    expect(clear.textContent).toContain('Clear')
    expect(clear.textContent).not.toContain('Tap again')
    click(clear)
    expect(onAction).not.toHaveBeenCalled()
    expect(armed()).toBe(true)

    // Using another row disarms it.
    click(rows[0]!)
    expect(onAction.mock.calls).toEqual([['/shake']])
    expect(armed()).toBe(false)
    onAction.mockClear()

    // Closing the popover disarms it.
    click(clear)
    expect(armed()).toBe(true)
    act(() => void document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })))
    expect(document.querySelector('[data-context-action="/clear"]')).toBeNull()
    click(button())
    const reopened = document.querySelector<HTMLElement>('[data-context-action="/clear"]')!
    expect(reopened.hasAttribute('data-confirm')).toBe(false)
    click(reopened)
    expect(onAction).not.toHaveBeenCalled()
  } finally {
    vi.useRealTimers()
  }
})

test('the composer row\'s icon buttons (+, tools, context, shield) share one box, one glyph size and one stroke', async () => {
  const { COMPOSER_ICON, COMPOSER_ICON_BOX, COMPOSER_ICON_STROKE } = await import('./composer-icons')
  expect(COMPOSER_ICON_BOX.split(' ')).toContain('size-9')
  expect(COMPOSER_ICON).toBe('size-[18px]')
  expect(COMPOSER_ICON_STROKE).toBe(2)
  const read = (f: string) => readFileSync(new URL(f, import.meta.url), 'utf8')
  const composer = read('./Composer.tsx')
  const mcp = read('./McpMenu.tsx')
  const meta = read('./ComposerMeta.tsx')
  expect(composer).toContain('<Plus className={COMPOSER_ICON} strokeWidth={COMPOSER_ICON_STROKE} />')
  expect(composer).toContain("className={cn(COMPOSER_ICON_BOX, 'text-fg-secondary', stage !== 'idle' && 'hidden')}")
  expect(mcp).toContain('<Wrench className={COMPOSER_ICON} strokeWidth={COMPOSER_ICON_STROKE} />')
  expect(mcp).toContain("className={cn(COMPOSER_ICON_BOX, 'text-fg-secondary', className)}")
  expect(meta).toContain('<Icon className={COMPOSER_ICON} strokeWidth={COMPOSER_ICON_STROKE}')
  expect(meta).toContain('<ContextRing pct={pct} className={COMPOSER_ICON} />')
  expect(meta.match(/className=\{cn\(COMPOSER_ICON_BOX, 'grid place-items-center/g)).toHaveLength(2)
  expect(mcp).not.toMatch(/size-5/)
})
