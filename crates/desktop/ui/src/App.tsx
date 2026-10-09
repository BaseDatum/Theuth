import { QueryObserver, useQueryClient } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRouter } from '@tanstack/react-router'
import { TrayIcon } from '@tauri-apps/api/tray'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { useShortcuts } from '@/lib/shortcuts'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { ATTENTION_KEY } from '@/components/hud/queries'
import { StartupScreen } from '@/components/launch/StartupScreen'
import { PairScreen } from '@/components/link/PairScreen'
import { Toaster } from '@/components/ui/sonner'
import { AlertNotifier } from '@/lib/alerts'
import { describe } from '@/lib/describe'
import { installDictationListeners, refreshStatus, setModel } from '@/lib/dictation'
import { companionAlive, onCompanionAlive, useCompanionAlive } from '@/lib/companion'
import { FocusTracker, Throttle, deriveFocus, typeCounts } from '@/lib/event-focus'
import { useMeetingHotkeys } from '@/lib/meeting-hotkeys'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { notifyIfHidden, primeNotifications } from '@/lib/notify'
import { leaves, usePanes } from '@/lib/panes'
import { installPagePreview } from '@/lib/page-preview'
import { container, isDesktop } from '@/lib/platform'
import { POPOUT, closePopout, popoutMeeting, popoutSession } from '@/lib/popout'
import { usePrivateOwner } from '@/lib/private'
import { queryKeys, useInvalidateOn } from '@/lib/rpc'
import { installSpeechListeners } from '@/lib/speech'
import * as talk from '@/lib/talk'
import {
  connection,
  isIsland,
  isOverlay,
  onCaptureHidden,
  onModelsProgress,
  onNavigate,
  onPointRefused,
  onQuitRequested,
  pointConfigure,
  quitAck,
  relaunch,
  setHiddenFromCapture,
  voiceKeyBind,
} from '@/lib/tauri'
import { startUpdateListener } from '@/lib/update'
import * as voiceKey from '@/lib/voice-key'
import { LOCAL, applyMeshStatus, hasLocal, hydrateEngines, useEngines, useNeedsPairing } from '@/link/engines'
import { autoPair, watchNodes } from '@/link/nodes'
import { LinkTransport } from '@/link/transport'
import { type Api, makeApi } from '@/rpc/api'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
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
/** 96 §5.3: a trail overlay (`overlay-<n>`) renders `/overlay` and nothing else — no engine, no listeners but `point:*`. */
const OVERLAY = isOverlay()
/** A popout (`lib/popout.ts`) is a second shell at the route in its URL: it connects like main but registers none of main's singletons. */
const AUX = ISLAND || OVERLAY || POPOUT !== null

const router = createRouter({
  routeTree,
  history: ISLAND || OVERLAY ? createMemoryHistory({ initialEntries: [ISLAND ? '/island' : '/overlay'] }) : undefined,
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
    // 76 §5.2: a talk another window (or this one before a reload) runs shows its live dot and orb here too.
    void talk.refreshStatus(api).catch(() => undefined)
    // 03 §0.4: the meeting's state, topics and brief stack; disabled meetings leave it empty.
    void hydrateMeeting(api).catch(() => undefined)
  })
}

/**
 * LIVE-GLYPHS (2026-10-07): the sidebar's job/PTY/reminder sets are fed by live events only; after a (re)connect, a
 * gap or an engine switch the engine's `list_live_counts` snapshot is the truth (an engine without the method warns).
 */
function seedLive(st: Stores): void {
  const api = st.connection.getState().api
  if (!api) return
  api.list_live_counts().then(
    (rows) => st.sessions.getState().seedLive(rows),
    (e: unknown) => console.warn('[live] list_live_counts failed', e),
  )
}

/**
 * 142 F4: `running`/`activity` and the subtree rows are advanced by live events only; a relay socket parked overnight
 * loses the `TurnFinished`s that clear them, and nothing re-read them — the sidebar showed subagents running until a
 * relaunch. Reset every slice to what the engine reports: `list_sessions` for the running marks, `list_live_counts`
 * for the job/PTY/reminder glyphs. Called on reconnect, on a `Lagged` resync and on every return to the foreground.
 */
function reseedLiveness(st: Stores): void {
  seedLive(st)
  const api = st.connection.getState().api
  if (!api) return
  api.list_sessions().then(
    (rows) => st.sessions.getState().seedRunning(rows),
    (e: unknown) => console.warn('[live] list_sessions failed', e),
  )
}

/** 142 F4: a foreground return with a socket that still looks `open` (iOS froze the page; the relay parked it) re-reads liveness — the gap's events are gone either way. */
function useForegroundReseed(st: Stores): void {
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === 'visible') reseedLiveness(st)
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => document.removeEventListener('visibilitychange', onVisible)
  }, [st])
}

/**
 * 129 D3: `Lagged` is a resync, not a reset. The gap is lost, so every slice is labelled stale and the mounted
 * chats re-settle through their `contextStale → settle()` path; `loaded` stays, so nothing on screen empties.
 * The session list is the one query refetched (`TurnStarted`/`TurnFinished` may have been dropped).
 */
function resync(st: Stores): void {
  const sessions = st.sessions.getState()
  sessions.resetLagged()
  sessions.markStale()
  void st.qc.invalidateQueries({ queryKey: queryKeys.sessions })
  reseedLiveness(st)
  if (useEngines.getState().active === st.id)
    window.dispatchEvent(new CustomEvent('theuth:reload-session', { detail: { settled: true } }))
}

/**
 * After a reconnect: the gap is lost, so refetch what the screen shows (25 §4.3) — a full page for the mounted
 * chats (the bare event; `running` may be wrong after a long gap and only a full page corrects it), the stale
 * mark for the rest (they settle on mount). Never `unload()` (129 D3). Communication state, peer directories
 * and request lists (50 §10) may have missed their change events too; a foreground return with a live
 * socket needs none of this (events kept flowing; stale queries refetch on focus).
 */
function reload(st: Stores): void {
  const sessions = st.sessions.getState()
  sessions.resetLagged()
  sessions.markStale()
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
  reseedLiveness(st)
  if (useEngines.getState().active === st.id)
    window.dispatchEvent(new Event('theuth:reload-session'))
}

/** 129 D4: a drain past this names the event types in its batch, so the next heavy event type names itself. */
const SLOW_DRAIN_MS = 50

/** `longtask` is Chromium-only (WKWebView has no such entry type): installed once, when the platform has it. */
let longTasksWatched = false
function watchLongTasks(): void {
  if (longTasksWatched || typeof PerformanceObserver === 'undefined' || !PerformanceObserver.supportedEntryTypes?.includes('longtask')) return
  longTasksWatched = true
  new PerformanceObserver((list) => {
    for (const e of list.getEntries()) console.warn('[events] long task', { ms: Math.round(e.duration) })
  }).observe({ entryTypes: ['longtask'] })
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
    const batch = queue.splice(0)
    const t0 = performance.now()
    dispatch(batch, st)
    const ms = performance.now() - t0
    if (ms > SLOW_DRAIN_MS) console.warn('[events] slow drain', { ms: Math.round(ms), events: batch.length, types: typeCounts(batch) })
  }
  watchLongTasks()
  st.attached = client.subscribe(null, null, (e) => {
    // The stream's first item (also after a reconnect's re-issue): the subscription is up, seed the live sets.
    if (e.type === 'PendingPrompts') seedLive(st)
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
 * The island and popout webviews register nothing: a second registration of a global shortcut throws, and the quit
 * handshake is the main window's. A popout only re-applies capture hiding, since it did not exist at the last toggle.
 */
function useTauri(): void {
  useEffect(() => {
    if (POPOUT && isDesktop()) void setHiddenFromCapture(useMeetingSettings.getState().hidden)
    if (AUX) return
    const unlistens = [
      onNavigate((path) => router.history.push(path)),
      onQuitRequested(() => void quit()),
      // `crates/desktop/src/models.rs`: one toast, updated in place, for the whole batch.
      onModelsProgress((p) => {
        const id = 'models'
        if (p.state === 'downloading')
          toast.loading(`Downloading speech models… ${Math.round((100 * p.done) / p.total)}%`, { id, description: p.file })
        else if (p.state === 'verifying') toast.loading('Verifying models…', { id, description: p.file })
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
  useVoiceKeyBinding()
  usePointConfigure()
}

/** ⌥Space (show/hide) and ⌥Esc (cancel all) as the user bound them (Settings › Shortcuts); desktop main window only. */
function useGlobalShortcuts(): void {
  const toggle = useShortcuts((s) => s.chords['toggle-window'])
  const cancel = useShortcuts((s) => s.chords['cancel-all'])
  useEffect(() => {
    if (AUX || !isDesktop()) return
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

/**
 * 96 §4.3: the voice key is a Rust-side global shortcut; the main webview tells the shell the chord at boot and on
 * every rebind (`""` when the key is off). A refused chord toasts and the store falls back to the last one that bound.
 */
function useVoiceKeyBinding(): void {
  const chord = useShortcuts((s) => s.chords['voice-key'])
  const enabled = voiceKey.useVoiceKeySettings((s) => s.enabled)
  // 149 D15/D22: the companion owns the chord while it runs — `""` unregisters; its exit re-binds `want`.
  const companion = useCompanionAlive()
  const bound = useRef<string | null>(null)
  useEffect(() => {
    if (AUX || !isDesktop()) return
    const want = enabled && !companion ? chord : ''
    voiceKeyBind(want).then(
      () => (bound.current = want),
      (e: unknown) => {
        toast.error(`Voice key: ${String(e)}`, { description: want ? `${want} stays unbound; the previous key still works` : undefined })
        if (bound.current !== null && enabled && bound.current !== '') useShortcuts.getState().set('voice-key', bound.current)
      },
    )
  }, [chord, enabled, companion])
  useEffect(() => {
    if (AUX || !isDesktop()) return
    const off = onPointRefused((p) => {
      if (p.reason !== 'chord taken') return
      toast.error('Voice key: another app holds this chord', {
        action: { label: 'Change the keys', onClick: () => router.history.push('/settings/shortcuts') },
      })
    })
    return () => void off.then((f) => f())
  }, [])
}

/** 96 (PANEL): the trail mode follows Settings › Voice key; sent after the binding at boot and on every change. */
function usePointConfigure(): void {
  const annotate = voiceKey.useVoiceKeySettings((s) => s.annotate)
  useEffect(() => {
    if (AUX || !isDesktop()) return
    void pointConfigure(annotate).catch((e: unknown) => console.warn('point_configure', e))
  }, [annotate])
}

/** `session` is on screen: the URL's chat, or any pane split beside it. */
function visible(session: string): boolean {
  if (router.state.location.pathname === `/c/${session}`) return true
  const { root } = usePanes.getState()
  return root !== null && leaves(root).includes(session)
}

/** 129 D3: one resync per 5 s (later `Lagged`s in the window coalesce into one trailing run); the toast once a minute. */
const RESYNC_MS = 5_000
const LAGGED_TOAST_MS = 60_000

/** 25 §4.5 notifications, the `Lagged` resync and the tray badge; per active engine. The island only resyncs; a popout also closes with its chat, PTY's session or meeting. */
function useEventSideEffects(st: Stores): void {
  const qc = useQueryClient()
  useEffect(() => {
    const resyncs = new Throttle(() => resync(st), RESYNC_MS, true)
    if (AUX) {
      const offs = [on('Lagged', () => resyncs.call())]
      const session = popoutSession(POPOUT)
      if (session !== null) offs.push(on('SessionDeleted', (e) => e.session === session && closePopout()))
      const meeting = popoutMeeting(POPOUT)
      if (meeting !== null) offs.push(on('Deleted', (e) => e.meeting === meeting && closePopout()))
      return () => {
        resyncs.cancel()
        for (const off of offs) off()
      }
    }
    const toasts = new Throttle(() => toast('Events dropped, resyncing'), LAGGED_TOAST_MS, false)
    // 130 D8: while the macOS companion is alive it owns banners and the menu bar count, so the
    // webview posts no notifications and clears its tray `●N` title; both come back when it exits.
    // 132 D7: the count is the attention inbox's length — the sidebar badge's number — read from the
    // `attention_list` cache the sidebar keeps live; approvals are one kind among its items.
    let companion = false
    // 134 W6: Windows shows no tray title, so the count also goes into the tooltip, worded like the companion's.
    const badge = (n: number) =>
      void TrayIcon.getById('main').then((t) => {
        void t?.setTitle(n ? `●${n}` : null)
        void t?.setTooltip(n === 0 ? 'theuth · nothing needs you' : n === 1 ? 'theuth · 1 needs you' : `theuth · ${n} need you`)
      })
    const setCompanion = (alive: boolean) => {
      if (alive === companion) return
      companion = alive
      if (isDesktop()) badge(alive ? 0 : (qc.getQueryData<AttentionItem[]>(ATTENTION_KEY)?.length ?? 0))
    }
    void companionAlive().then(setCompanion)
    // 155 §7.6: the engine decides whether (`item.alert`); this window posts one notification per `alert.id`.
    const alerts = new AlertNotifier((title, body) => void notifyIfHidden(title, body), visible)
    const offs = [
      onCompanionAlive(setCompanion),
      on('Lagged', () => {
        resyncs.call()
        toasts.call()
      }),
      on(
        'ApprovalRequested',
        (e) => {
          if (companion) return
          alerts.legacy(`approval:${e.id}`, `Approval: ${e.tool}`, describe(e.tool, e.args).join('\n'))
        },
      ),
      on('TurnFinished', (e) => {
        if (companion || visible(e.session)) return
        const slice = st.sessions.getState().sessions[e.session]
        const summary = qc
          .getQueryData<SessionSummary[]>(queryKeys.sessions)
          ?.find((s) => s.id === e.session)
        if ((slice?.parent ?? summary?.parent) != null) return
        alerts.legacy(`finished:${e.session}:`, 'Turn finished', slice?.title ?? summary?.title ?? '')
      }),
      on('BrowserShowLive', (e) => {
        if (!visible(e.session)) router.history.push(`/c/${e.session}`)
      }),
      on('SessionDeleted', (e) => {
        const to = usePanes.getState().close(e.session)
        if (to) router.history.push(`/c/${to}`)
      }),
      // `enabled: false`: observe the cached list, never fetch it here (the sidebar's `useAttention` owns the fetch).
      new QueryObserver<AttentionItem[]>(qc, { queryKey: ATTENTION_KEY, enabled: false }).subscribe((r) => {
        if (companion || !r.data) return
        alerts.list(r.data)
        if (isDesktop()) badge(r.data.length)
      }),
    ]
    return () => {
      resyncs.cancel()
      alerts.dispose()
      for (const off of offs) off()
    }
  }, [st, qc])
}

/** 129 §3.4: the focus call waits this long after a route, pane, popout or desk change. */
const FOCUS_DEBOUNCE_MS = 100

/**
 * 129 §3.4: tell the engine which sessions this window renders live text for (`deriveFocus`); deltas for the
 * rest stay on the server. Sent on every `open` (server state is per connection) and when the set changes;
 * a session leaving the set is marked stale (its deltas are skipped from here), one entering re-settles through
 * the chat route's `contextStale → settle()`. An engine from before 129 answers -32601: ignored.
 */
function useEventFocus(st: Stores): void {
  useEffect(() => {
    const tracker = new FocusTracker()
    let timer = 0
    const send = () => {
      timer = 0
      const { api, state } = st.connection.getState()
      if (!api || state !== 'open') return
      const { root } = usePanes.getState()
      const next = deriveFocus({
        pathname: router.state.location.pathname,
        leaves: root ? leaves(root) : [],
        popout: popoutSession(POPOUT),
        desk: voiceKey.useVoiceKey.getState().desk,
      })
      const { send: changed, left } = tracker.update(next)
      if (!changed) return
      for (const s of left) st.sessions.getState().markStale(s)
      api.set_event_focus(next).catch(() => undefined)
    }
    const schedule = () => {
      if (!timer) timer = window.setTimeout(send, FOCUS_DEBOUNCE_MS)
    }
    const offs = [
      router.subscribe('onResolved', schedule),
      usePanes.subscribe((s, prev) => s.root !== prev.root && schedule()),
      voiceKey.useVoiceKey.subscribe((s, prev) => s.desk !== prev.desk && schedule()),
      st.connection.subscribe((s, prev) => {
        if (s.state !== 'open' || prev.state === 'open') return
        tracker.reset()
        send()
      }),
    ]
    send()
    return () => {
      clearTimeout(timer)
      for (const off of offs) off()
    }
  }, [st])
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
  useEventFocus(scope(engine))
  useForegroundReseed(scope(engine))
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

/** 96 §5.3: an overlay webview is a canvas and three listeners — no engines, no stores, no toaster. */
export function App() {
  return OVERLAY ? <RouterProvider router={router} /> : <Main />
}

function Main() {
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
  // 96 §5: the island webview drives dictation for the voice panel; the overlays install nothing but their route's `point:*`.
  useEffect(() => (OVERLAY ? undefined : installDictationListeners()), [])
  useEffect(() => (ISLAND ? voiceKey.install() : undefined), [])
  useEffect(() => (ISLAND ? undefined : talk.installTalkListeners()), [])
  useEffect(installMeetingListeners, [])
  // 120 §11.3: only the main window renders `page_preview` requests; a popout or the island would race it.
  useEffect(() => (AUX ? undefined : installPagePreview()), [])
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
  // 51 §8.3: one router-level owner; `reload()` marks slices stale only, the private record (memory-only) survives a reconnect.
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
