import '../settings/test-dom'
import { act, createElement as h, type ReactElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { notify } from '@/lib/notify'
import type { RpcClient } from '@/rpc/client'
import { scope } from '@/store/engine-scope'
import { useEventRate } from '@/store/event-rate'
import { useNotices } from '@/store/notices'
import { dotOf } from './model'
import { PhoneStatusLine } from './PhoneStatusLine'
import { StatusBar } from './StatusBar'

const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate }))
const layout = vi.hoisted(() => ({ mobile: false }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => layout.mobile }))

const st = scope('local')
const retryNow = vi.fn()

beforeEach(() => {
  useNotices.getState().clear()
  useEventRate.getState().reset()
  st.connection.setState({ state: 'open', retryAt: null, attempt: 0, info: null, client: { retryNow } as unknown as RpcClient })
  st.approvals.setState({ pending: [] })
  st.asks.setState({ pending: [] })
  st.secretRequests.setState({ pending: [] })
  navigate.mockClear()
  retryNow.mockClear()
  layout.mobile = false
})
afterEach(() => {
  document.body.innerHTML = ''
  vi.useRealTimers()
})

const mount = async (el: ReactElement = h(StatusBar)) => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(el))
  return { host, root, bar: () => host.querySelector<HTMLElement>('[data-status-bar]')! }
}
const dot = (host: HTMLElement) => host.querySelector<HTMLElement>('.status-dot')!
const button = (host: ParentNode, text: string) => [...host.querySelectorAll('button')].find((b) => b.textContent?.includes(text))!
const pending = (n: number) => Array.from({ length: n }, (_, i) => ({ id: `p${i}`, session: 's1' })) as never[]

test('the dot matrix: connection and needs-you only', () => {
  expect(dotOf('open', 0, 12)).toEqual({ tone: 'ok', pulse: true })
  expect(dotOf('open', 0, 0)).toEqual({ tone: 'ok', pulse: false })
  expect(dotOf('open', 2, 12)).toEqual({ tone: 'attn', pulse: false })
  expect(dotOf('connecting', 0, 0)).toEqual({ tone: 'idle', pulse: true })
  expect(dotOf('reconnecting', 3, 50)).toEqual({ tone: 'down', pulse: false })
  expect(dotOf('closed', 0, 0)).toEqual({ tone: 'down', pulse: false })
})

test('open: malachite, ochre with needs-you; an error notice never turns it red', async () => {
  const { host, root } = await mount()
  expect(dot(host).dataset.tone).toBe('ok')
  await act(async () => void notify.error('boom'))
  expect(dot(host).dataset.tone).toBe('ok')
  await act(async () => st.approvals.setState({ pending: pending(2) }))
  await act(async () => st.asks.setState({ pending: pending(1) }))
  expect(dot(host).dataset.tone).toBe('attn')
  await act(async () => root.unmount())
})

test('reconnecting: carnelian, "<engine> disconnected", the countdown and Retry now; the rate reads –', async () => {
  vi.useFakeTimers()
  try {
    const { host, root, bar } = await mount()
    await act(async () => st.connection.setState({ state: 'reconnecting', retryAt: Date.now() + 3_000, attempt: 2, info: { name: 'Studio' } as never }))
    expect(dot(host).dataset.tone).toBe('down')
    expect(bar().textContent).toContain('Studio disconnected')
    expect(bar().textContent).toContain('Reconnecting in 3s · attempt 2 · Retry now')
    expect(bar().textContent).toContain('– ev/s')
    await act(async () => void vi.advanceTimersByTime(1_000))
    expect(bar().textContent).toContain('Reconnecting in 2s')
    await act(async () => button(host, 'Retry now').click())
    expect(retryNow).toHaveBeenCalledOnce()
    await act(async () => root.unmount())
  } finally {
    vi.useRealTimers()
  }
})

test('the slot shows the newest notice with its action; acting dismisses it', async () => {
  vi.useFakeTimers()
  try {
    const onClick = vi.fn()
    const { host, root, bar } = await mount()
    await act(async () => void notify('first'))
    await act(async () => void notify.success('Saved as a note', { action: { label: 'Open', onClick } }))
    const slot = host.querySelector<HTMLElement>('[data-notice-slot]')!
    expect(slot.textContent).toContain('done')
    expect(slot.textContent).toContain('Saved as a note')
    expect(slot.textContent).not.toContain('first')
    await act(async () => button(slot, 'Open').click())
    expect(onClick).toHaveBeenCalledOnce()
    expect(useNotices.getState().current).toBeNull()
    // It fades out, then leaves the slot.
    expect(host.querySelector('[data-notice-slot]')!.className).toContain('opacity-0')
    await act(async () => void vi.advanceTimersByTime(250))
    expect(bar().textContent).not.toContain('Saved as a note')
    await act(async () => root.unmount())
  } finally {
    vi.useRealTimers()
  }
})

test('needs-you badge counts approvals, asks and secrets, and opens the inbox', async () => {
  const { host, root, bar } = await mount()
  expect(bar().textContent).not.toContain('need you')
  await act(async () => {
    st.approvals.setState({ pending: pending(1) })
    st.asks.setState({ pending: pending(1) })
    st.secretRequests.setState({ pending: pending(1) })
  })
  expect(bar().textContent).toContain('3 need you')
  await act(async () => button(host, 'need you').click())
  expect(navigate).toHaveBeenCalledWith({ to: '/attention', search: true })
  await act(async () => root.unmount())
})

test('the rate reads the mean of the last two seconds', async () => {
  const { root, bar } = await mount()
  await act(async () => useEventRate.setState({ buckets: [...Array(28).fill(0), 1_400, 1_598] }))
  expect(bar().textContent).toContain('1.5k ev/s')
  await act(async () => root.unmount())
})

test('the unread-error chip opens the history, which reads them', async () => {
  const { host, root } = await mount()
  await act(async () => void notify.error('Could not register ⌥Space'))
  await act(async () => void notify('later'))
  const chip = host.querySelector<HTMLButtonElement>('button[aria-label="1 unread errors"]')!
  expect(chip.textContent).toBe('● 1')
  await act(async () => chip.click())
  const popover = document.querySelector<HTMLElement>('[data-notice-popover]')!
  expect(popover.textContent).toContain('Recent')
  expect(popover.textContent).toContain('Could not register ⌥Space')
  expect(useNotices.getState().unreadErrors).toBe(0)
  expect(host.querySelector('button[aria-label$="unread errors"]')).toBeNull()
  await act(async () => root.unmount())
})

const slotText = (host: HTMLElement) => host.querySelector('[data-notice-slot]')?.textContent ?? ''

test('the bar\'s clock expires a notice on time, then a superseded sticky one returns to the slot', async () => {
  vi.useFakeTimers()
  const { host, root } = await mount()
  await act(async () => void notify.loading('Downloading speech models… 40%', { id: 'models' }))
  await act(async () => void notify('cancelled 2 sessions'))
  expect(slotText(host)).toContain('cancelled 2 sessions')
  await act(async () => void vi.advanceTimersByTime(3_999))
  expect(slotText(host)).toContain('cancelled 2 sessions')
  await act(async () => void vi.advanceTimersByTime(1))
  expect(slotText(host)).toContain('Downloading speech models… 40%')
  // Same id updates in place: the slot keeps its element (no replayed slide).
  const el = host.querySelector('[data-notice-slot]')
  await act(async () => void notify.loading('Downloading speech models… 41%', { id: 'models' }))
  expect(host.querySelector('[data-notice-slot]')).toBe(el)
  expect(slotText(host)).toContain('41%')
  await act(async () => root.unmount())
})

test('× on an offer runs its cancel, then dismisses it for good (the OnboardingHost shape)', async () => {
  const start = vi.fn()
  const later = vi.fn()
  const { host, root } = await mount()
  await act(async () => void notify('Take the two-minute tour?', { duration: Infinity, action: { label: 'Start', onClick: start }, cancel: { label: 'Not now', onClick: later } }))
  await act(async () => host.querySelector<HTMLButtonElement>('button[aria-label="Not now"]')!.click())
  expect(later).toHaveBeenCalledOnce()
  expect(start).not.toHaveBeenCalled()
  expect(useNotices.getState().current).toBeNull()
  expect(useNotices.getState().history[0]?.dismissed).toBe(true)
  await act(async () => void notify('later'))
  useNotices.getState().expire(Date.now() + 4_000)
  expect(useNotices.getState().current).toBeNull()
  await act(async () => root.unmount())
})

test('down: a current notice keeps the slot with the countdown after it; no deadline reads "Reconnecting…"', async () => {
  const { host, root, bar } = await mount()
  await act(async () => void notify.error('Voice key: another app holds this chord'))
  await act(async () => st.connection.setState({ state: 'reconnecting', retryAt: Date.now() + 5_000, attempt: 1 }))
  expect(slotText(host)).toContain('Voice key: another app holds this chord')
  expect(bar().querySelector('[data-reconnect]')!.textContent).toMatch(/^Reconnecting in [45]s · attempt 1 · Retry now$/)
  await act(async () => st.connection.setState({ state: 'closed', retryAt: null, attempt: 0 }))
  expect(bar().querySelector('[data-reconnect]')!.textContent).toBe('Reconnecting…')
  expect(bar().textContent).not.toContain('Retry now')
  expect(bar().textContent).toContain('– ev/s')
  await act(async () => root.unmount())
})

test('the guest bar is notices only', async () => {
  const { host, root, bar } = await mount(h(StatusBar, { guest: true }))
  await act(async () => void notify('You can now only view this document'))
  expect(slotText(host)).toContain('You can now only view this document')
  expect(bar().querySelector('.status-dot')).toBeNull()
  expect(bar().textContent).not.toContain('ev/s')
  await act(async () => root.unmount())
})

test('the phone gets the line, never the bar: rate, then a notice, then offline', async () => {
  vi.useFakeTimers()
  layout.mobile = true
  await act(async () => useEventRate.setState({ buckets: [...Array(28).fill(0), 1_500, 1_500] }))
  const { host, root } = await mount(h('div', null, h(StatusBar), h(PhoneStatusLine)))
  const line = () => host.querySelector<HTMLElement>('[data-phone-status]')!
  expect(host.querySelector('[data-status-bar]')).toBeNull()
  expect(line().textContent).toBe('1.5k ev/s')
  expect(line().querySelector('.status-dot')!.className).toContain('size-[5px]')
  await act(async () => void notify.success('Saved as a note'))
  expect(line().textContent).toBe('Saved as a note')
  await act(async () => void notify.dismiss())
  await act(async () => st.connection.setState({ state: 'reconnecting', retryAt: Date.now() + 2_500, attempt: 1 }))
  // The notice fades for 200 ms before the line returns.
  await act(async () => void vi.advanceTimersByTime(250))
  expect(line().textContent).toBe('offline · retry 3s')
  await act(async () => root.unmount())
})

test('tapping the phone line opens the sheet: engine, rate, needs-you and the history, read', async () => {
  layout.mobile = true
  st.connection.setState({ info: { name: 'Studio' } as never })
  st.approvals.setState({ pending: pending(2) })
  const { host, root } = await mount(h(PhoneStatusLine))
  await act(async () => void notify.error('boom'))
  expect(useNotices.getState().unreadErrors).toBe(1)
  await act(async () => host.querySelector<HTMLButtonElement>('[data-phone-status]')!.click())
  const sheet = document.querySelector<HTMLElement>('[data-notice-popover]')!
  expect(sheet.textContent).toContain('Studio')
  expect(sheet.textContent).toContain('2 need you')
  expect(sheet.textContent).toContain('Recent')
  expect(sheet.textContent).toContain('boom')
  expect(useNotices.getState().unreadErrors).toBe(0)
  await act(async () => root.unmount())
})
