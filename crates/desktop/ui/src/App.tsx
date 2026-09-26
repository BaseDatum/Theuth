import { useQueryClient } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRouter } from '@tanstack/react-router'
import { TrayIcon } from '@tauri-apps/api/tray'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { useShortcuts } from '@/lib/shortcuts'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { StartupScreen } from '@/components/launch/StartupScreen'
import { PairScreen } from '@/components/link/PairScreen'
import { Toaster } from '@/components/ui/sonner'
import { describe } from '@/lib/describe'
import { installDictationListeners, refreshStatus, setModel } from '@/lib/dictation'
import { useMeetingHotkeys } from '@/lib/meeting-hotkeys'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { notifyIfHidden, primeNotifications } from '@/lib/notify'
import { leaves, usePanes } from '@/lib/panes'
import { container, isDesktop } from '@/lib/platform'
import { usePrivateOwner } from '@/lib/private'
import { queryKeys, useInvalidateOn } from '@/lib/rpc'
import { installSpeechListeners } from '@/lib/speech'
import {
  connection,
  isIsland,
  onCaptureHidden,
  onModelsProgress,
  onNavigate,
  onQuitRequested,
  quitAck,
  relaunch,
  setHiddenFromCapture,
} from '@/lib/tauri'
import { startUpdateListener } from '@/lib/update'
import { LOCAL, applyMeshStatus, hasLocal, hydrateEngines, useEngines, useNeedsPairing } from '@/link/engines'
import { autoPair, watchNodes } from '@/link/nodes'
import { LinkTransport } from '@/link/transport'
import { type Api, makeApi } from '@/rpc/api'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { RpcClient, RpcError } from '@/rpc/client'
import type { WireEvent } from '@/rpc/events'
import { routeTree } from '@/routeTree.gen'
import { useConnection } from '@/store/connection'
import { EngineProvider, type Stores, dropScope, scope } from '@/store/engine-scope'
import { hydrateMeeting, installMeetingListeners } from '@/store/meeting'
import { dispatch, on, useSessions } from '@/store/sessions'

/** 03 §0.1: the island webview loads the same bundle and lives at `/island`; the browser history cannot read a window label. */
const ISLAND = isIsland()

const router = createRouter({
  routeTree,
  history: ISLAND ? createMemoryHistory({ initialEntries: ['/island'] }) : undefined,
})

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

/** rAF is paused while the window is hidden; a timer keeps events (and notifications) flowing then. */
const HIDDEN_DRAIN_MS = 250

/** Set by the local boot: 18 §3 — quitting `shutdown`s a daemon we started, not one we reused. */
let owned = false

/** 25 §4.3 quit: RPC `shutdown` if owned (errors ignored: the socket may close first), then `quit_ack`. */
async function quit(): Promise<void> {
  const api = scope(LOCAL).connection.getState().api
  if (owned && api) await api.shutdown().catch(() => undefined)
  await quitAck()
}

/**
 * `info` into the connection store, then the engine's nodes (32 §8.2) and the dictation model (54 §0.5; a refusal — voice disabled, relay peer — marks the model failed, which hides every field mic and dims the composer's).
 * A mesh row also re-reads `mesh_status` (69 §4): whichever engine is the hub now names the mesh and its members.
 */
function loadInfo(st: Stores, api: Api): Promise<void> {
  return api.info().then((info) => {
    st.connection.getState().setInfo(info)
    void autoPair(st.id, api)
    if (useEngines.getState().engines.find((h) => h.id === st.id)?.kind === 'mesh') void api.mesh_status().then((s) => applyMeshStatus(st.id, s)).catch(() => undefined)
    void api.dictation_prewarm().then(setModel, (e: unknown) => setModel({ state: 'failed', message: e instanceof Error ? e.message : String(e) }))
    void refreshStatus(api).catch(() => undefined)
    // 03 §0.4: the meeting's state, topics and brief stack; disabled meetings leave it empty.
    void hydrateMeeting(api).catch(() => undefined)
  })
}

/**
 * After a reconnect or `Lagged`: the gap is lost, so refetch what the screen shows (25 §4.3).
 * The chat route takes the event when mounted; `unload` covers a route that mounts later
 * (a local reconnect shows the startup screen meanwhile). Communication state, peer directories and
 * request lists (50 §10) may have missed their change events too; a foreground return with a live
 * socket needs none of this (events kept flowing; stale queries refetch on focus).
 */
function reload(st: Stores): void {
  const sessions = st.sessions.getState()
  sessions.resetLagged()
  sessions.unload()
  void st.qc.invalidateQueries({ queryKey: queryKeys.sessions })
  void st.qc.invalidateQueries({ queryKey: queryKeys.knowledge })
  void st.qc.invalidateQueries({ queryKey: ['communication'] })
  void st.qc.invalidateQueries({ queryKey: ['peers'] })
  void st.qc.invalidateQueries({ queryKey: ['peer-messages'] })
  void st.qc.invalidateQueries({ queryKey: queryKeys.a2a })
  void st.qc.invalidateQueries({ queryKey: queryKeys.searchAll })
  void st.qc.invalidateQueries({ queryKey: ['session-cost'] })
  void st.qc.invalidateQueries({ queryKey: ['terminals'] })
  const api = st.connection.getState().api
  if (api) void loadInfo(st, api).catch(() => undefined)
  if (useEngines.getState().active === st.id)
    window.dispatchEvent(new Event('theuth:reload-session'))
}

/**
 * `subscribe_events(null, null)` for the active engine (25 §6.1, 31 §7.3). Events
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
 * Connect one engine: build its client, mirror its state into its connection
 * store, subscribe if it is the active one and load `info` on every `open` (a
 * link engine may be offline for a while first). `signal` aborts (StrictMode
 * remount, relaunch, engine removed) and closes the client.
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
    // -32601 on `info` = an older `t --headless` already owns the configured port.
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
        // Toasts for the engine on screen; a mesh row's hub failover (69 decision 6) shows the switcher's
        // "reconnecting…" chip instead — the new hub answers on the same id within seconds.
        const { active, engines } = useEngines.getState()
        const shown = active === st.id && engines.find((h) => h.id === st.id)?.kind !== 'mesh'
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
      if (useEngines.getState().active === st.id) attach(st)
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

/**
 * Tray / menu navigation, the quit handshake (macOS); notification permission (iOS); mounted once.
 * The island webview registers nothing: a second registration of a global shortcut throws, and the quit handshake is the main window's.
 */
function useTauri(): void {
  useEffect(() => {
    if (ISLAND) return
    const unlistens = [
      onNavigate((path) => router.history.push(path)),
      onQuitRequested(() => void quit()),
      // `crates/desktop/src/models.rs`: one toast, updated in place, for the whole batch.
      onModelsProgress((p) => {
        const id = 'models'
        if (p.state === 'downloading')
          toast.loading(`Downloading speech models… ${Math.round((100 * p.done) / p.total)}%`, { id, description: p.file })
        else if (p.state === 'ready') {
          toast.success('Speech models ready', { id, description: undefined })
          // The daemon reloads lazily but the UI gates the mic on `model.state`; ask again so the button unlocks without a restart.
          const api = useConnection.getState().api
          if (api) void api.dictation_prewarm().then(setModel).catch(() => undefined)
        } else toast.error(`Speech models: ${p.message}`, { id, description: undefined })
      }),
    ]
    const onQuit = () => void quit()
    window.addEventListener('theuth:quit', onQuit)
    void primeNotifications()
    // The updater swaps a `.app` bundle (update.rs `bundle_dir`): macOS only.
    if (container() === 'macos') startUpdateListener()
    // 53 decision 9: the eye applies to both windows from startup, not only on toggle; the command
    // exists on every desktop OS and no-ops where `island_capabilities().hide_from_share` is false.
    if (isDesktop()) void setHiddenFromCapture(useMeetingSettings.getState().hidden)
    return () => {
      for (const u of unlistens) void u.then((f) => f())
      window.removeEventListener('theuth:quit', onQuit)
    }
  }, [])
  useGlobalShortcuts()
}

/** ⌥Space (show/hide) and ⌥Esc (cancel all) as the user bound them (Settings › Shortcuts); desktop main window only. */
function useGlobalShortcuts(): void {
  const toggle = useShortcuts((s) => s.chords['toggle-window'])
  const cancel = useShortcuts((s) => s.chords['cancel-all'])
  useEffect(() => {
    if (ISLAND || !isDesktop()) return
    void register(toggle, async (e) => {
      if (e.state !== 'Pressed') return
      const win = getCurrentWindow()
      if (await win.isVisible()) await win.hide()
      else {
        await win.show()
        await win.setFocus()
      }
    }).catch(() => toast.error(`Could not register ${toggle}; another app holds it`))
    void register(cancel, (e) => {
      if (e.state === 'Pressed') void cancelAll()
    }).catch(() => toast.error(`Could not register ${cancel}; another app holds it`))
    return () => void unregister([toggle, cancel]).catch(() => undefined)
  }, [toggle, cancel])
}

/** `session` is on screen: the URL's chat, or any pane split beside it. */
function visible(session: string): boolean {
  if (router.state.location.pathname === `/c/${session}`) return true
  const { root } = usePanes.getState()
  return root !== null && leaves(root).includes(session)
}

/** 25 §4.5 notifications, the `Lagged` reload and the tray badge; per active engine. The island only reloads. */
function useEventSideEffects(st: Stores): void {
  const qc = useQueryClient()
  useEffect(() => {
    if (ISLAND) return on('Lagged', () => reload(st))
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
        if (visible(e.session)) return
        const slice = st.sessions.getState().sessions[e.session]
        const summary = qc
          .getQueryData<SessionSummary[]>(queryKeys.sessions)
          ?.find((s) => s.id === e.session)
        if ((slice?.parent ?? summary?.parent) != null) return
        void notifyIfHidden('Turn finished', slice?.title ?? summary?.title ?? '')
      }),
      on('BrowserShowLive', (e) => {
        if (!visible(e.session)) router.history.push(`/c/${e.session}`)
      }),
      on('SessionDeleted', (e) => {
        const to = usePanes.getState().close(e.session)
        if (to) router.history.push(`/c/${to}`)
      }),
      st.approvals.subscribe((s, prev) => {
        if (!isDesktop() || s.pending.length === prev.pending.length) return
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

/**
 * Everything under the active engine's provider: the router once connected, the startup screen before.
 * A link engine stays on the router through a reconnect: iOS kills the socket on every background,
 * so a resume would otherwise swap in the startup screen and remount the router each time (31 §7.3).
 * `refused` needs the user (re-pair or forget), so that still gets the screen.
 */
function Shell({ engine, onRelaunch }: { engine: string; onRelaunch(): void }) {
  const refused = useEngines((s) => s.presence[engine] === 'refused')
  const ready = useConnection(
    (s) => s.info !== null && (s.state === 'open' || (engine !== LOCAL && !refused)),
  )
  useInvalidateOn()
  useEventSideEffects(scope(engine))
  return ready ? (
    <RouterProvider router={router} />
  ) : (
    <StartupScreen onRelaunch={onRelaunch} onQuit={() => void quit()} />
  )
}

/** Link clients by engine id; `AbortController` closes one when the engine is removed. */
const links = new Map<string, AbortController>()

/** The chat each engine was showing when the user switched away, restored on the way back (`/` otherwise). */
const lastChat = new Map<string, string>()

export function App() {
  const [attempt, setAttempt] = useState(0)
  const hydrated = useEngines((s) => s.hydrated)
  const engines = useEngines((s) => s.engines)
  const active = useEngines((s) => s.active)
  const needsPairing = useNeedsPairing()
  /** The `active` last shown once IndexedDB was read; `null` before, so the stored value does not count as a switch. */
  const shown = useRef<string | null>(null)

  useEffect(() => void hydrateEngines(), [])
  useEffect(watchNodes, [])
  useEffect(installSpeechListeners, [])
  useEffect(() => (ISLAND ? undefined : installDictationListeners()), [])
  useEffect(installMeetingListeners, [])
  // The shell flips both windows; each webview mirrors the value into its own settings copy.
  useEffect(() => {
    const off = onCaptureHidden((h) => useMeetingSettings.getState().setHidden(h))
    return () => void off.then((f) => f())
  }, [])

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

  // One LinkTransport client per paired engine; all connect (presence), only the active one subscribes.
  // A node its parent introduced while offline (`pairedAt: 0`, 32 §8.2) has no client until it pairs.
  useEffect(() => {
    if (!hydrated) return
    for (const h of engines) {
      if (links.has(h.id) || h.pairedAt === 0) continue
      const ctl = new AbortController()
      links.set(h.id, ctl)
      boot(scope(h.id), async () => new RpcClient(new LinkTransport(h)), ctl.signal)
    }
    for (const [id, ctl] of links) {
      if (engines.some((h) => h.id === id)) continue
      ctl.abort()
      links.delete(id)
      dropScope(id)
    }
  }, [hydrated, engines])

  // Switching moves the subscription; the tree below remounts under the new provider. The engine left
  // behind misses every event from here, so its transcripts go stale until it is shown again (48 §5)
  // — on a real switch only, not a StrictMode re-run of this effect.
  useEffect(() => {
    const st = scope(active)
    attach(st)
    return () => {
      detach(st)
      if (useEngines.getState().active !== st.id) st.sessions.getState().unload()
    }
  }, [active])

  // A switch restores that engine's last chat, else `/` (the tree below remounts under the new provider);
  // the hydration read keeps deep links like `/link#…`.
  useEffect(() => {
    if (!hydrated) return
    if (shown.current !== null && shown.current !== active) {
      const { pathname, href } = router.state.location
      if (pathname.startsWith('/c/')) lastChat.set(shown.current, href)
      else lastChat.delete(shown.current)
      // Panes name sessions of the engine being left.
      usePanes.setState({ root: null, focused: null, drag: null })
      router.history.push(lastChat.get(active) ?? '/')
    }
    shown.current = active
  }, [hydrated, active])

  useTauri()
  useMeetingHotkeys()
  // 51 §8.3: one router-level owner; `reload()`'s `unload()` marks slices only, the private record (memory-only) survives a reconnect.
  usePrivateOwner(router)

  return (
    <>
      {needsPairing ? (
        <PairScreen />
      ) : (
        <EngineProvider key={active} engine={active}>
          <Shell engine={active} onRelaunch={() => setAttempt((n) => n + 1)} />
        </EngineProvider>
      )}
      <Toaster />
    </>
  )
}
