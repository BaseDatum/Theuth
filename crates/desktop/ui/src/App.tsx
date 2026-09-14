import { type QueryClient, useQueryClient } from '@tanstack/react-query'
import { RouterProvider, createRouter } from '@tanstack/react-router'
import { TrayIcon } from '@tauri-apps/api/tray'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { StartupScreen } from '@/components/launch/StartupScreen'
import { Toaster } from '@/components/ui/sonner'
import { describe } from '@/lib/describe'
import { notifyIfHidden } from '@/lib/notify'
import { queryKeys, useInvalidateOn } from '@/lib/rpc'
import { type Connection, connection, isTauri, onNavigate, onQuitRequested, quitAck, relaunch } from '@/lib/tauri'
import { makeApi } from '@/rpc/api'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { RpcClient } from '@/rpc/client'
import type { WireEvent } from '@/rpc/events'
import { routeTree } from '@/routeTree.gen'
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { dispatch, on, useSessions } from '@/store/sessions'

const router = createRouter({ routeTree })

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

/** rAF is paused while the window is hidden; a timer keeps events (and notifications) flowing then. */
const HIDDEN_DRAIN_MS = 250
const TOGGLE_SHORTCUT = 'Alt+Space'

/** Set by `bootstrap`: 18 §3 — quitting `shutdown`s a daemon we started, not one we reused. */
let owned = false

/** 25 §4.3 quit: RPC `shutdown` if owned (errors ignored: the socket may close first), then `quit_ack`. */
async function quit(): Promise<void> {
  const api = useConnection.getState().api
  if (owned && api) await api.shutdown().catch(() => undefined)
  await quitAck()
}

/** After a reconnect or `Lagged`: the gap is lost, so refetch what the screen shows (25 §4.3). */
function reload(qc: QueryClient): void {
  useSessions.getState().resetLagged()
  void qc.invalidateQueries({ queryKey: queryKeys.sessions })
  void useConnection
    .getState()
    .api?.info()
    .then((info) => useConnection.getState().setInfo(info))
  window.dispatchEvent(new Event('wth:reload-session'))
}

/**
 * Open the socket, subscribe once (25 §6.1) and load `info`. Events are queued
 * and drained into one `dispatch` per frame; this is the only place they enter
 * the stores. `signal` aborts (StrictMode remount, relaunch) and closes the client.
 */
function bootstrap(get: () => Promise<Connection>, qc: QueryClient, signal: AbortSignal): void {
  const { setConnState, setClient, setInfo, setError } = useConnection.getState()
  setError(undefined)
  setClient(null, null)
  setInfo(null)
  let client: RpcClient | undefined
  signal.addEventListener('abort', () => client?.close())
  void (async () => {
    try {
      const conn = await get()
      if (signal.aborted) return
      owned = conn.owned
      client = new RpcClient(conn.url, conn.token)
      const api = makeApi(client)
      let lost = false
      client.onState((s) => {
        setConnState(s)
        if (s === 'reconnecting' && !lost) {
          lost = true
          toast('Connection lost, reconnecting…')
        } else if (s === 'open' && lost) {
          lost = false
          toast('Reconnected')
          reload(qc)
        }
      })
      await client.connect()
      setClient(client, api)

      const queue: WireEvent[] = []
      let frame = 0
      let timer = 0
      const drain = () => {
        cancelAnimationFrame(frame)
        clearTimeout(timer)
        frame = timer = 0
        dispatch(queue.splice(0))
      }
      await client.subscribe(null, null, (e) => {
        queue.push(e)
        if (frame) return
        frame = requestAnimationFrame(drain)
        timer = window.setTimeout(drain, HIDDEN_DRAIN_MS)
      })
      setInfo(await api.info())
    } catch (e) {
      if (!signal.aborted) setError(e instanceof Error ? e.message : String(e))
    }
  })()
}

/** Tray / menu navigation, the quit handshake, ⌥Space and the tray badge; mounted once. */
function useTauri(): void {
  useEffect(() => {
    const unlistens = [onNavigate((path) => router.history.push(path)), onQuitRequested(() => void quit())]
    const onQuit = () => void quit()
    window.addEventListener('wth:quit', onQuit)
    if (isTauri()) {
      void register(TOGGLE_SHORTCUT, async (e) => {
        if (e.state !== 'Pressed') return
        const win = getCurrentWindow()
        if (await win.isVisible()) await win.hide()
        else {
          await win.show()
          await win.setFocus()
        }
      }).catch(() => toast.error(`Could not register ${TOGGLE_SHORTCUT}; another app holds it`))
    }
    const offBadge = useApprovals.subscribe((s, prev) => {
      if (!isTauri() || s.pending.length === prev.pending.length) return
      void TrayIcon.getById('main').then((t) => t?.setTitle(s.pending.length ? `●${s.pending.length}` : null))
    })
    return () => {
      for (const u of unlistens) void u.then((f) => f())
      window.removeEventListener('wth:quit', onQuit)
      if (isTauri()) void unregister(TOGGLE_SHORTCUT)
      offBadge()
    }
  }, [])
}

/** 25 §4.5 notifications and the `Lagged` reload; mounted once. */
function useEventSideEffects(qc: QueryClient): void {
  useEffect(() => {
    const offs = [
      on('Lagged', () => {
        reload(qc)
        toast('Events dropped, reloaded')
      }),
      on('ApprovalRequested', (e) => void notifyIfHidden(`Approval: ${e.tool}`, describe(e.tool, e.args).join('\n'))),
      on('TurnFinished', (e) => {
        if (router.state.location.pathname === `/c/${e.session}`) return
        const slice = useSessions.getState().sessions[e.session]
        const summary = qc.getQueryData<SessionSummary[]>(queryKeys.sessions)?.find((s) => s.id === e.session)
        if ((slice?.parent ?? summary?.parent) != null) return
        void notifyIfHidden('Turn finished', slice?.title ?? summary?.title ?? '')
      }),
    ]
    return () => {
      for (const off of offs) off()
    }
  }, [qc])
}

export function App() {
  const qc = useQueryClient()
  const ready = useConnection((s) => s.state === 'open' && s.info !== null)
  const [attempt, setAttempt] = useState(0)

  useEffect(() => {
    const ctl = new AbortController()
    bootstrap(attempt === 0 ? connection : relaunch, qc, ctl.signal)
    return () => ctl.abort()
  }, [qc, attempt])

  useTauri()
  useInvalidateOn()
  useEventSideEffects(qc)

  return (
    <>
      {ready ? (
        <RouterProvider router={router} />
      ) : (
        <StartupScreen onRelaunch={() => setAttempt((n) => n + 1)} onQuit={() => void quit()} />
      )}
      <Toaster />
    </>
  )
}
