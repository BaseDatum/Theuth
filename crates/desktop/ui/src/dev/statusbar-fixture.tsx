// 217 visual check, dev only: `bunx vite` then open `/src/dev/statusbar-fixture.html` (dark; `?light` for light).
// The real StatusBar, NoticeHistory and PhoneStatusLine with no engine. The notice and event-rate stores are global,
// so every state runs in its own iframe (`?state=<name>`), which seeds the stores its own way; a 390 px frame is
// under the phone breakpoint, so the phone states get the phone line and the desktop ones the bar.
import '@/styles.css'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { PhoneStatusLine } from '@/components/status/PhoneStatusLine'
import { StatusBar } from '@/components/status/StatusBar'
import { TooltipProvider } from '@/components/ui/tooltip'
import { notify } from '@/lib/notify'
import type { Info } from '@/rpc/bindings/method.rpc_2dcompose.output'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { useEventRate } from '@/store/event-rate'

const query = new URLSearchParams(location.search)
const light = query.has('light')
document.documentElement.classList.toggle('dark', !light)
const state = query.get('state')

const st = active()
const noop = () => {}

function connected() {
  st.connection.setState({ state: 'open', info: { name: 'Studio Mac' } as Info })
}

/** About `perSecond` events a second, in uneven bursts so the sparkline moves; 30 s of history so it starts full. */
function stream(perSecond: number) {
  const jitter = () => 0.6 + Math.random() * 0.8
  useEventRate.setState({ buckets: Array.from({ length: 30 }, () => Math.round(perSecond * jitter())) })
  const id = setInterval(() => useEventRate.getState().tick(Math.round((perSecond / 4) * jitter())), 250)
  return () => clearInterval(id)
}

/** A backoff that counts down, then schedules the next attempt further out; Retry now starts over at 2 s. */
function reconnecting() {
  const arm = (attempt: number, ms: number) => st.connection.setState({ state: 'reconnecting', attempt, retryAt: Date.now() + ms })
  st.connection.setState({
    info: { name: 'Studio Mac' } as Info,
    client: { retryNow: () => arm(st.connection.getState().attempt + 1, 2_000) } as unknown as RpcClient,
  })
  arm(3, 8_000)
  const id = setInterval(() => {
    const { retryAt, attempt } = st.connection.getState()
    if (retryAt !== null && retryAt <= Date.now()) arm(attempt + 1, Math.min(30_000, 1_000 * 2 ** attempt))
  }, 250)
  return () => clearInterval(id)
}

function pending(n: number) {
  st.approvals.setState({ pending: Array.from({ length: n }, (_, i) => ({ id: `a${i}`, session: 's1' })) as never[] })
}

function history() {
  notify.success('Speech models ready')
  notify.warning('Events dropped, resyncing')
  notify('cancelled 3 sessions')
  notify.error('Voice key: another app holds this chord', { action: { label: 'Change the keys', onClick: noop } })
  notify.success('Saved as a note', { description: 'Notes › Status bar review\nfrom the chat "217 status bar"', action: { label: 'Open', onClick: noop } })
}

const SEED: Record<string, () => (() => void) | void> = {
  open: () => (connected(), stream(1_500)),
  idle: connected,
  done: () => {
    connected()
    notify.success('Saved as a note', { description: 'Notes › Status bar review, from the chat "217 status bar"', action: { label: 'Open', onClick: noop } })
  },
  'needs-you': needsYou,
  needs: needsYou,
  error: () => {
    connected()
    notify.error('Could not register ⌥Space; another app holds it')
    return stream(300)
  },
  reconnecting,
  history: () => {
    connected()
    history()
    return stream(1_500)
  },
  'phone-rate': () => (connected(), stream(1_500)),
  'phone-notice': () => {
    connected()
    notify.success('Saved as a note', { duration: Infinity })
    return stream(1_500)
  },
  'phone-offline': reconnecting,
  'phone-sheet': () => {
    connected()
    pending(2)
    history()
    return stream(1_500)
  },
}

function needsYou() {
  connected()
  pending(2)
  return stream(40)
}

// Seeded before the first render, so no frame ever shows the unseeded defaults.
if (state) SEED[state]?.()

function Scenario({ name }: { name: string }) {
  // Opened from the first render, not by a click after it.
  const open = name === 'history' || name === 'phone-sheet'
  if (name.startsWith('phone-')) {
    // The home-indicator band an iPhone reports; the frame has none of its own.
    document.documentElement.style.setProperty('--safe-bottom', '34px')
    return (
      <div className="relative flex h-dvh flex-col bg-background">
        <div className="flex h-11 shrink-0 items-center justify-center border-b border-border-subtle text-sm font-medium">217 status bar</div>
        <div className="flex min-h-0 flex-1 flex-col justify-end gap-3 p-4 text-sm">
          <p className="self-end rounded-2xl bg-layer-2 px-3 py-2">Save this as a note</p>
          <p className="text-fg-secondary">Done — it is in Notes as “Status bar review”.</p>
        </div>
        <div className="mx-3 mb-2 flex h-11 shrink-0 items-center rounded-2xl border border-border-subtle px-3 text-sm text-fg-tertiary">Message…</div>
        <PhoneStatusLine defaultOpen={open} />
        <span aria-hidden className="pointer-events-none absolute bottom-2 left-1/2 h-[5px] w-[134px] -translate-x-1/2 rounded-full bg-foreground/80" />
      </div>
    )
  }
  return (
    <div className="flex h-dvh flex-col bg-background">
      <div className="min-h-0 flex-1" />
      <StatusBar defaultOpen={open} />
    </div>
  )
}

const DESKTOP: [string, string, number][] = [
  ['open', 'Open, ~1.5k ev/s, live sparkline', 22],
  ['idle', 'Open, idle at 0', 22],
  ['done', 'A done notice with an action', 22],
  ['needs-you', 'Needs you: 2', 22],
  ['error', 'An error notice and the unread chip', 22],
  ['reconnecting', 'Reconnecting, with the countdown', 22],
  ['history', 'History popover open', 520],
]
const PHONE: [string, string][] = [
  ['phone-rate', 'Phone: rate'],
  ['phone-notice', 'Phone: notice'],
  ['phone-offline', 'Phone: offline'],
  ['phone-sheet', 'Phone: sheet open'],
]

function Gallery() {
  const src = (name: string) => `${location.pathname}?state=${name}${light ? '&light' : ''}`
  return (
    <main className="min-h-dvh overflow-auto bg-background p-6 text-foreground">
      <header className="mb-6 flex items-baseline gap-4">
        <h1 className="text-lg font-medium">217 · status bar</h1>
        <a className="text-sm text-fg-tertiary underline" href={light ? location.pathname : `${location.pathname}?light`}>
          {light ? 'Dark' : 'Light'}
        </a>
      </header>
      <section className="mb-10 flex flex-col gap-5">
        {DESKTOP.map(([name, label, h]) => (
          <figure key={name} className="flex flex-col gap-1.5">
            <figcaption className="text-xs text-fg-tertiary">{label}</figcaption>
            <iframe title={label} src={src(name)} className="w-full max-w-[1100px] border border-border-subtle" style={{ height: h }} />
          </figure>
        ))}
      </section>
      <section className="flex flex-wrap gap-6">
        {PHONE.map(([name, label]) => (
          <figure key={name} className="flex flex-col gap-1.5">
            <figcaption className="text-xs text-fg-tertiary">{label}</figcaption>
            <iframe title={label} src={src(name)} width={390} height={844} className="rounded-[36px] border-[6px] border-border-strong" />
          </figure>
        ))}
      </section>
    </main>
  )
}

const root = createRootRoute({
  component: () => (
    <TooltipProvider>
      {state ? <Scenario name={state} /> : <Gallery />}
    </TooltipProvider>
  ),
})
const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }) })

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>,
)
