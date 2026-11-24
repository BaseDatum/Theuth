import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { notify } from '@/lib/notify'
import type { RpcClient } from '@/rpc/client'
import { scope } from '@/store/engine-scope'
import { useEventRate } from '@/store/event-rate'
import { useNotices } from '@/store/notices'
import { dotOf } from './model'
import { StatusBar } from './StatusBar'

const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate }))

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
})
afterEach(() => {
  document.body.innerHTML = ''
})

const mount = async () => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(StatusBar)))
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
  expect(navigate).toHaveBeenCalledWith({ to: '/attention' })
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
