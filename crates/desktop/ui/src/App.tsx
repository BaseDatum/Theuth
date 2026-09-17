import { useQueryClient } from '@tanstack/react-query'
import { RouterProvider, createRouter } from '@tanstack/react-router'
import { TrayIcon } from '@tauri-apps/api/tray'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { StartupScreen } from '@/components/launch/StartupScreen'
import { PairScreen } from '@/components/link/PairScreen'
import { Toaster } from '@/components/ui/sonner'
import { describe } from '@/lib/describe'
import { notifyIfHidden, primeNotifications } from '@/lib/notify'
import { container } from '@/lib/platform'
import { queryKeys, useInvalidateOn } from '@/lib/rpc'
import { connection, onNavigate, onQuitRequested, quitAck, relaunch } from '@/lib/tauri'
import { startUpdateListener } from '@/lib/update'
import { LOCAL, hasLocal, hydrateHarnesses, useHarnesses, useNeedsPairing } from '@/link/harnesses'
import { autoPair, watchNodes } from '@/link/nodes'
import { LinkTransport } from '@/link/transport'
import { type Api, makeApi } from '@/rpc/api'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { RpcClient, RpcError } from '@/rpc/client'
import type { WireEvent } from '@/rpc/events'
import { routeTree } from '@/routeTree.gen'
import { useConnection } from '@/store/connection'
import { HarnessProvider, type Stores, dropScope, scope } from '@/store/harness-scope'
import { dispatch, on, useSessions } from '@/store/sessions'

const router = createRouter({ routeTree })

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

/** Injected by vite `define` from tauri.conf.json. */
declare const __APP_VERSION__: string

/** rAF is paused while the window is hidden; a timer keeps events (and notifications) flowing then. */
const HIDDEN_DRAIN_MS = 250
const TOGGLE_SHORTCUT = 'Alt+Space'
const CANCEL_SHORTCUT = 'Alt+Escape'

/** Set by the local boot: 18 §3 — quitting `shutdown`s a daemon we started, not one we reused. */
let owned = false

/** 25 §4.3 quit: RPC `shutdown` if owned (errors ignored: the socket may close first), then `quit_ack`. */
async function quit(): Promise<void> {
  const api = scope(LOCAL).connection.getState().api
  if (owned && api) await api.shutdown().catch(() => undefined)
  await quitAck()
}

/** `info` into the connection store, then the harness's nodes (32 §8.2). */
function loadInfo(st: Stores, api: Api): Promise<void> {
  return api.info().then((info) => {
    st.connection.getState().setInfo(info)
    void autoPair(st.id, api)
  })
}

/**
 * After a reconnect or `Lagged`: the gap is lost, so refetch what the screen shows (25 §4.3).
 * On a reconnect the router is still unmounted (`Shell` shows the startup screen while
 * `reconnecting`), so the event below finds no listener; `unload` makes the chat route
 * fetch again when it mounts. `Lagged` arrives with the route mounted and takes the event.
 */
function reload(st: Stores): void {
  const sessions = st.sessions.getState()
  sessions.resetLagged()
  sessions.unload()
  void st.qc.invalidateQueries({ queryKey: queryKeys.sessions })
  const api = st.connection.getState().api
  if (api) void loadInfo(st, api).catch(() => undefined)
  if (useHarnesses.getState().active === st.id)
    window.dispatchEvent(new Event('wth:reload-session'))
}

/**
 * `subscribe_events(null, null)` for the active harness (25 §6.1, 31 §7.3). Events
 * are queued and drained into one `dispatch` per frame; this is the only place
 * they enter the stores. Issued on `open` when the client is not connected yet.
 */
function attach(st: Stores): void {
  const client = st.connection.getState().client
  if (!client || st.attached) return
  const queue: WireEvent[] = []
  let frame = 0
  let timer = 0
  const drain = () => {
    cancelAnimationFrame(frame)
    clearTimeout(timer)
    frame = timer = 0
    dispatch(queue.splice(0), st)
  }
  st.attached = client.subscribe(null, null, (e) => {
    queue.push(e)
    if (frame) return
    frame = requestAnimationFrame(drain)
    timer = window.setTimeout(drain, HIDDEN_DRAIN_MS)
  })
  st.attached.catch(() => undefined)
}

function detach(st: Stores): void {
  const attached = st.attached
  st.attached = null
  void attached?.then((off) => off()).catch(() => undefined)
}

/**
 * Connect one harness: build its client, mirror its state into its connection
 * store, subscribe if it is the active one and load `info` on every `open` (a
 * link harness may be offline for a while first). `signal` aborts (StrictMode
 * remount, relaunch, harness removed) and closes the client.
 */
function boot(st: Stores, make: () => Promise<RpcClient>, signal: AbortSignal): void {
  const conn = st.connection.getState()
  conn.setError(undefined)
  conn.setClient(null, null)
  conn.setInfo(null)
  let client: RpcClient | undefined
  signal.addEventListener('abort', () => {
    detach(st)
    client?.close()
  })
  const fail = (e: unknown) => {
    if (signal.aborted) return
    // -32601 on `info` = an older `wth --headless` already owns the configured port.
    conn.setError(
      e instanceof RpcError && e.code === -32601
        ? `The daemon already running on this port is older than this app (no "info" method). Quit it and relaunch.`
        : e instanceof Error
          ? e.message
          : String(e),
    )
  }
  void (async () => {
    try {
      client = await make()
      if (signal.aborted) {
        client.close()
        return
      }
      const api = makeApi(client)
      let lost = false
      client.onState((s) => {
        conn.setConnState(s)
        const shown = useHarnesses.getState().active === st.id
        if (s === 'reconnecting' && !lost) {
          lost = true
          if (shown) toast('Connection lost, reconnecting…')
        } else if (s === 'open') {
          conn.setError(undefined)
          if (lost) {
            lost = false
            if (shown) toast('Reconnected')
            reload(st)
          } else loadInfo(st, api).catch(fail)
        }
      })
      conn.setClient(client, api)
      if (useHarnesses.getState().active === st.id) attach(st)
      await client.connect()
    } catch (e) {
      fail(e)
    }
  })()
}

/** ⌥Esc: `cancel` every session with a turn in flight, one toast for the lot. */
async function cancelAll(): Promise<void> {
  const api = useConnection.getState().api
  if (!api) return
  const running = Object.values(useSessions.getState().sessions).filter((s) => s.running)
  await Promise.all(running.map((s) => api.cancel(s.id).catch(() => undefined)))
  if (running.length) toast(`cancelled ${running.length} sessions`)
}

/** Tray / menu navigation, the quit handshake, ⌥Space and ⌥Esc (macOS); notification permission (iOS); mounted once. */
function useTauri(): void {
  useEffect(() => {
    const unlistens = [
      onNavigate((path) => router.history.push(path)),
      onQuitRequested(() => void quit()),
    ]
    const onQuit = () => void quit()
    window.addEventListener('wth:quit', onQuit)
    const desktop = container() === 'macos'
    void primeNotifications()
    if (desktop) {
      startUpdateListener()
      void register(TOGGLE_SHORTCUT, async (e) => {
        if (e.state !== 'Pressed') return
        const win = getCurrentWindow()
        if (await win.isVisible()) await win.hide()
        else {
          await win.show()
          await win.setFocus()
        }
      }).catch(() => toast.error(`Could not register ${TOGGLE_SHORTCUT}; another app holds it`))
      void register(CANCEL_SHORTCUT, (e) => {
        if (e.state === 'Pressed') void cancelAll()
      }).catch(() => toast.error(`Could not register ${CANCEL_SHORTCUT}; another app holds it`))
    }
    return () => {
      for (const u of unlistens) void u.then((f) => f())
      window.removeEventListener('wth:quit', onQuit)
      if (desktop) void unregister([TOGGLE_SHORTCUT, CANCEL_SHORTCUT])
    }
  }, [])
}

/** 25 §4.5 notifications, the `Lagged` reload and the tray badge; per active harness. */
function useEventSideEffects(st: Stores): void {
  const qc = useQueryClient()
  useEffect(() => {
    const offs = [
      on('Lagged', () => {
        reload(st)
        toast('Events dropped, reloaded')
      }),
      on(
        'ApprovalRequested',
        (e) => void notifyIfHidden(`Approval: ${e.tool}`, describe(e.tool, e.args).join('\n')),
      ),
      on('TurnFinished', (e) => {
        if (router.state.location.pathname === `/c/${e.session}`) return
        const slice = st.sessions.getState().sessions[e.session]
        const summary = qc
          .getQueryData<SessionSummary[]>(queryKeys.sessions)
          ?.find((s) => s.id === e.session)
        if ((slice?.parent ?? summary?.parent) != null) return
        void notifyIfHidden('Turn finished', slice?.title ?? summary?.title ?? '')
      }),
      on('BrowserShowLive', (e) => {
        if (router.state.location.pathname !== `/c/${e.session}`)
          router.history.push(`/c/${e.session}`)
      }),
      st.approvals.subscribe((s, prev) => {
        if (container() !== 'macos' || s.pending.length === prev.pending.length) return
        void TrayIcon.getById('main').then((t) =>
          t?.setTitle(s.pending.length ? `●${s.pending.length}` : null),
        )
      }),
    ]
    return () => {
      for (const off of offs) off()
    }
  }, [st, qc])
}

/** Everything under the active harness's provider: the router once connected, the startup screen before. */
function Shell({ harness, onRelaunch }: { harness: string; onRelaunch(): void }) {
  const ready = useConnection((s) => s.state === 'open' && s.info !== null)
  useInvalidateOn()
  useEventSideEffects(scope(harness))
  return ready ? (
    <RouterProvider router={router} />
  ) : (
    <StartupScreen onRelaunch={onRelaunch} onQuit={() => void quit()} />
  )
}

/** Link clients by harness id; `AbortController` closes one when the harness is removed. */
const links = new Map<string, AbortController>()

/** The chat each harness was showing when the user switched away, restored on the way back (`/` otherwise). */
const lastChat = new Map<string, string>()

export function App() {
  const [attempt, setAttempt] = useState(0)
  const hydrated = useHarnesses((s) => s.hydrated)
  const harnesses = useHarnesses((s) => s.harnesses)
  const active = useHarnesses((s) => s.active)
  const needsPairing = useNeedsPairing()
  /** The `active` last shown once IndexedDB was read; `null` before, so the stored value does not count as a switch. */
  const shown = useRef<string | null>(null)

  useEffect(() => void hydrateHarnesses(), [])
  useEffect(watchNodes, [])

  // The desktop's own daemon (25's launch path), relaunched on demand from the startup screen.
  useEffect(() => {
    if (!hasLocal()) return
    const ctl = new AbortController()
    const get = attempt === 0 ? connection : relaunch
    boot(
      scope(LOCAL),
      async () => {
        const c = await get()
        owned = c.owned
        return RpcClient.local(c.url, c.token)
      },
      ctl.signal,
    )
    return () => ctl.abort()
  }, [attempt])

  // One LinkTransport client per paired harness; all connect (presence), only the active one subscribes.
  // A node its parent introduced while offline (`pairedAt: 0`, 32 §8.2) has no client until it pairs.
  useEffect(() => {
    if (!hydrated) return
    for (const h of harnesses) {
      if (links.has(h.id) || h.pairedAt === 0) continue
      const ctl = new AbortController()
      links.set(h.id, ctl)
      boot(scope(h.id), async () => new RpcClient(new LinkTransport(h)), ctl.signal)
    }
    for (const [id, ctl] of links) {
      if (harnesses.some((h) => h.id === id)) continue
      ctl.abort()
      links.delete(id)
      dropScope(id)
    }
  }, [hydrated, harnesses])

  // Switching moves the subscription; the tree below remounts under the new provider.
  useEffect(() => {
    const st = scope(active)
    attach(st)
    return () => detach(st)
  }, [active])

  // A switch restores that harness's last chat, else `/` (the tree below remounts under the new provider);
  // the hydration read keeps deep links like `/link#…`.
  useEffect(() => {
    if (!hydrated) return
    if (shown.current !== null && shown.current !== active) {
      const { pathname, href } = router.state.location
      if (pathname.startsWith('/c/')) lastChat.set(shown.current, href)
      else lastChat.delete(shown.current)
      router.history.push(lastChat.get(active) ?? '/')
    }
    shown.current = active
  }, [hydrated, active])

  useTauri()

  return (
    <>
      {needsPairing ? (
        <PairScreen />
      ) : (
        <HarnessProvider key={active} harness={active}>
          <Shell harness={active} onRelaunch={() => setAttempt((n) => n + 1)} />
        </HarnessProvider>
      )}
      <Toaster />
      {/* Above the safe area on iOS; nothing under it is clickable-through. */}
      <span className="pointer-events-none fixed right-2 bottom-[max(0.25rem,var(--safe-bottom))] z-50 text-[10px] leading-none text-muted-foreground/60 select-none">
        v{__APP_VERSION__}
      </span>
    </>
  )
}
