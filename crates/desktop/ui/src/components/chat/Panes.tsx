import { layout as L, useOptionalView, ViewType, Workspace, type Keymap, type MenuEntry, type WorkspaceHandle } from '@danfessler/trellis-react'
import '@danfessler/trellis/style.css'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useSidebarIsMobile } from '@/components/ui/sidebar'
import { useIsMobile } from '@/hooks/use-mobile'
import { type PressDragProps, usePressDrag } from '@/hooks/use-press-drag'
import { active } from '@/store/engine-scope'
import { TileHost } from '@/components/tiles/host'
import { useEngines } from '@/link/engines'
import {
  attachWorkspace,
  CHAT,
  chatLeaves,
  DRAFT_TYPE,
  type DragPayload,
  hasPendingDrop,
  isDraft,
  isFrame,
  newDraft,
  picked,
  prepareStored,
  reopenClosed,
  shown,
  storedChatLeaves,
  PERSIST,
  STORAGE_KEY,
  tileType,
  usePanes,
  VERSION,
  WEB,
  zone,
} from '@/lib/panes'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import { isMacDesktop } from '@/lib/hotkeys'
import { label, matches } from '@/lib/chord'
import { queryKeys, useApi } from '@/lib/rpc'
import { useShortcuts } from '@/lib/shortcuts'
import { closeAllTiles } from '@/lib/tiles'
import { cn } from '@/lib/utils'
import { crossLayout } from '@/lib/web-arrival'
import { useWebViews } from '@/lib/web-views'
import { useQuerySupport } from '@/rpc/capabilities'
import { PhoneWebView } from '@/components/chat/PhoneWebView'
import { talkPaneToKeep } from '@/components/tiles/open'
import * as talk from '@/lib/talk'

/**
 * Drag a sidebar row onto the panes (`usePressDrag` owns the gesture and its DOM glue): a drop on a pane's edge band
 * splits it there; on its middle or its panel's tab bar the chat joins that panel as its active tab (`targetAt`). A
 * click still shows the chat in the focused pane (`show`). Spread the result on the
 * row's `Link`; a press that became a drag never navigates, selects or opens the row's context menu. `id === null`
 * is the New chat rail item: each drag carries a fresh draft id, and a drop opens a draft pane (`NewChatPane`)
 * without moving the URL. In the phone Sheet fingers never drag: phones do not tile.
 */
function openWorkspaceForDrop(
  navigate: ReturnType<typeof useNavigate>,
  fallbackPath?: string,
): void {
  const { leaves, focused } = usePanes.getState()
  const sessions = active().qc.getQueryData<SessionSummary[]>(queryKeys.sessions) ?? []
  const listed = new Set(sessions.map((s) => s.id))
  const ok = (s: string): boolean => !isDraft(s) && (listed.size === 0 || listed.has(s))
  const laid = chatLeaves(leaves).filter(ok)
  const target =
    (focused !== null && laid.includes(focused) ? focused : null) ??
    laid[0] ??
    storedChatLeaves().find(ok) ??
    sessions.find((s) => s.parent === null && s.desk == null && !s.archived)?.id ??
    sessions[0]?.id ??
    null
  if (target) void navigate({ to: '/c/$session', params: { session: target } })
  else if (fallbackPath) void navigate({ href: fallbackPath })
  else void navigate({ to: '/new' })
}

export function useSessionDrag(id: string | null, title: string) {
  const navigate = useNavigate()
  const isMobile = useIsMobile()
  return usePressDrag({
    disabledForFinger: isMobile,
    onStart: (x, y) => usePanes.getState().startDrag(id ?? newDraft(), title, x, y),
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd(drop) {
      const dropped = usePanes.getState().endDrag(drop)
      if (!dropped) return
      if (!isDraft(dropped) && !isFrame(dropped)) {
        void navigate({ to: '/c/$session', params: { session: dropped } })
      } else if (hasPendingDrop()) {
        openWorkspaceForDrop(navigate)
      }
    },
  }).props
}

/** Drag any app view (`/tasks`, `/notebooks`, `/costs`, `/settings/appearance`, …) from the sidebar or a section band into a pane. */
export function useViewDrag(path: string, title: string) {
  const navigate = useNavigate()
  const isMobile = useIsMobile()
  return usePressDrag({
    disabledForFinger: isMobile,
    onStart: (x, y) => usePanes.getState().startPayloadDrag({ kind: 'view', path, title }, title, x, y),
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd(drop) {
      const dropped = usePanes.getState().endDrag(drop)
      if (dropped && hasPendingDrop()) openWorkspaceForDrop(navigate, path)
    },
  }).props
}

/** Drag a generic tile (`plugin`, etc.) from the sidebar into a pane. */
export function useTileDrag(tileKind: string, props: unknown, title: string, key?: string) {
  const navigate = useNavigate()
  const isMobile = useIsMobile()
  return usePressDrag({
    disabledForFinger: isMobile,
    onStart: (x, y) => usePanes.getState().startPayloadDrag({ kind: 'tile', tileKind, props, key, title }, title, x, y),
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd(drop) {
      const dropped = usePanes.getState().endDrag(drop)
      if (dropped && hasPendingDrop()) openWorkspaceForDrop(navigate)
    },
  }).props
}

/**
 * Drag a chat's tray item — a dash beside the grabber or a tile of the opened tray (an artifact, a terminal, a
 * whiteboard, the browser, …) — into a pane, as a sidebar row: the same `usePressDrag` gesture (mouse: move; finger:
 * hold, then move), so it works with a finger on the iPad as with the mouse. `payload` null: the item has no pane form
 * (a list behind a menu), and nothing is spread. `prepare` runs just before a drop is placed (a web frame is recorded
 * in its store first, which `payloadLeafAndSpec` reads). A press that became a drag never runs the item's click.
 */
export function usePayloadDrag(payload: DragPayload | null, title: string, prepare?: () => void): PressDragProps | Record<string, never> {
  const navigate = useNavigate()
  const isMobile = useIsMobile()
  const { props } = usePressDrag({
    disabledForFinger: isMobile,
    onStart: (x, y) => {
      if (payload) usePanes.getState().startPayloadDrag(payload, title, x, y)
    },
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd(drop) {
      if (drop && usePanes.getState().drag?.target) prepare?.()
      const dropped = usePanes.getState().endDrag(drop)
      if (dropped && hasPendingDrop()) openWorkspaceForDrop(navigate)
    },
  })
  return payload ? props : {}
}

/** The drag ghost and the snapped drop zone; mounted once in the shell, renders nothing between drags. A drop that
 * makes the chat a tab of the panel under it (its middle or its tab bar) outlines that whole panel with the new tab
 * drawn at its top; an edge drop shades the half the split will give it. */
export function DragOverlay() {
  const drag = usePanes((s) => s.drag)
  if (!drag) return null
  const tab = drag.target?.edge === 'tab'
  return (
    <>
      {drag.target && (
        <div
          data-drop={drag.target.edge}
          className="pointer-events-none fixed z-50 rounded-xl border-2 border-fg-accent bg-fg-accent/10 transition-[left,top,width,height] duration-100"
          style={zone(drag.target)}
        >
          {tab && (
            <div className="absolute top-1 left-1 flex max-w-56 items-center gap-1.5 truncate rounded-md border border-fg-accent bg-popover px-2 py-0.5 text-xs shadow-sm">
              <span className="truncate">{drag.title}</span>
              <span className="shrink-0 text-fg-tertiary">· new tab</span>
            </div>
          )}
        </div>
      )}
      <div
        className="pointer-events-none fixed z-50 max-w-56 truncate rounded-md border bg-popover px-2 py-1 text-xs shadow-md"
        style={{ left: drag.x + 12, top: drag.y + 12 }}
      >
        {drag.title}
      </div>
    </>
  )
}

type Render = (leaf: string, first: boolean) => ReactNode
/** Each view type's tab icon (`chat`, `draft`, `web`, `tile:<kind>`). */
type Icons = Readonly<Record<string, ReactNode>>
const NO_ICONS: Icons = {}

/**
 * One view's content, whatever its kind: the sidebar drop target for its pane (`data-pane`). A press inside selects it
 * (`usePanes.selected`, any kind) and, for a chat, focuses it and moves the URL there (a draft has no URL); a frame (a
 * web view, 205 D5, or a generic tile) is selected only — the URL stays on the chat. The selected pane is lit, the
 * others dim. A press on a control in a chat while a frame is selected changes nothing, so a surface that control
 * opens joins the selected frame's panel as a tab. The subtree's surfaces open as tiles beside this view (`useTileHost`).
 */
function ViewBody({ render }: { render: Render }) {
  const id = useOptionalView()?.id ?? ''
  const first = usePanes((s) => s.first === id)
  const dim = usePanes((s) => {
    if (s.leaves.length < 2) return false
    const lit = s.selected !== null && s.leaves.includes(s.selected) ? s.selected : s.focused
    return lit !== id
  })
  return (
    <div
      data-pane={id}
      className={cn('absolute inset-0 flex min-h-0 min-w-0 flex-col', dim && 'opacity-80')}
      onPointerDownCapture={(e) => {
        const { leaves, selected } = usePanes.getState()
        const control = e.target instanceof Element && e.target.closest('button,a,[role=button],[role=menuitem]') !== null
        const frameLit = isFrame(selected) && selected !== id && leaves.includes(selected)
        if (control && !isFrame(id) && frameLit) return
        picked(id)
      }}
    >
      <TileHost.Provider value={id}>{render(id, first)}</TileHost.Provider>
    </div>
  )
}

/** Built-in panel menu items that need what this app does not offer: floating and hiding (there is no dock). */
const UNUSED_MENU: Record<string, true> = { float: true, dock: true, hide: true }
/** The panel menu: Trellis's own items this app uses, then Recently closed (newest first; ⌘⇧T reopens the last). */
const panelMenu = (entries: MenuEntry[]): MenuEntry[] => {
  const { closed } = usePanes.getState()
  const chord = label(useShortcuts.getState().chords['reopen-closed'])
  const recent: MenuEntry = {
    id: 'recently-closed',
    label: 'Recently closed',
    disabled: closed.length === 0,
    items: [...closed].reverse().map((c, i) => ({ label: c.title, shortcut: i === 0 ? chord : undefined, run: () => void reopenClosed(c.leaf) })),
  }
  return [...entries.filter((e) => e === 'separator' || !UNUSED_MENU[e.id ?? '']), 'separator', recent]
}

/**
 * Zoom navigation, `focus` mode: a pane (or a part of the layout) is framed — maximised to the whole workspace at its
 * real size, the rest kept mounted off screen — by double-clicking its tab bar or header (`useViewDrag`), the panel
 * menu's Maximize or ⌘⌥↩; ⌘⌥↑ shows everything again, ⌘⌥← / ⌘⌥→ go back and forth, Escape steps out while focus is
 * on the workspace itself (a tab, or the workspace after a double-click), never from inside a view (the composer, a
 * terminal) nor from a dialog (portalled outside the workspace, whose key listener never sees it). The other keymap
 * entries (F6, ⌘⌥[ / ⌘⌥], ⌘⌥W) were already live and clash with nothing in the app. `free` mode (pinch, ⌘⌥-drag)
 * stays off: it would leave panes live at any size (a PTY, a VM session, a native web view resized with every
 * pinch) and its ⌘⇧ / ⌘⌃ drag chords take presses from the content.
 * Trellis's ⌘⇧↩ for framing collides with a notebook's Run all, hence ⌘⌥↩.
 */
const KEYMAP: Keymap = { 'frame.toggle': 'Mod+Alt+Enter' }

/**
 * The tiling layout: a Trellis workspace with one view type per leaf kind, its layout persisted on this machine. It
 * mounts once the engine's session list is known (or cannot be asked for), after the persisted layout dropped what is
 * gone (`prepareStored`), so no stale chat mounts and loads. Trellis keeps each view's content mounted while it moves,
 * docks or resizes (iframes, terminals, VNC canvases and composers keep their state).
 */
function Tiled({ session, kinds, icons, render }: { session: string; kinds: readonly string[]; icons: Icons; render: Render }) {
  const navigate = useNavigate()
  const api = useApi()
  const canList = useQuerySupport(['list_sessions'])
  const sessions = useQuery({ enabled: canList, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const types = useMemo(() => [CHAT, DRAFT_TYPE, WEB, ...kinds.map(tileType)], [kinds])
  const ready = !canList || sessions.data !== undefined || sessions.isError
  if (!ready) return null
  return (
    <Prepared
      session={session}
      types={types}
      icons={icons}
      sessions={sessions.data?.map((s) => s.id) ?? null}
      render={render}
      navigate={(s) => void navigate({ to: '/c/$session', params: { session: s } })}
    />
  )
}

function Prepared(p: { session: string; types: string[]; icons: Icons; sessions: string[] | null; render: Render; navigate(session: string): void }) {
  const { session, types, icons, sessions, render, navigate } = p
  const engine = useEngines((s) => s.active)
  // Once, before the workspace reads the persisted layout (its mount, below).
  useState(() => PERSIST && prepareStored({ engine, sessions: sessions && new Set(sessions), types: new Set(types) }))
  const [initial] = useState(() => L.view(CHAT, { id: session, params: { engine } }))
  const go = useRef(navigate)
  go.current = navigate
  // ⌘⇧T (Settings › Shortcuts): the last closed tile reopens where it was.
  const reopen = useShortcuts((s) => s.chords['reopen-closed'])
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!matches(reopen, e) || reopenClosed() === null) return
      e.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [reopen])
  const detach = useRef<(() => void) | null>(null)
  const ref = useCallback((ws: WorkspaceHandle | null) => {
    detach.current?.()
    detach.current = ws ? attachWorkspace(ws, (s) => go.current(s)) : null
  }, [])
  return (
    <>
      <CornerTabs />
      <Workspace
        ref={ref}
        label="Panes"
        theme="light"
        floating={false}
        navigation="focus"
        keymap={KEYMAP}
        panelMenu={panelMenu}
        storageKey={PERSIST ? STORAGE_KEY : undefined}
        version={VERSION}
        defaultLayout={initial}
        onMissingType={() => 'drop'}
      >
        {types.map((t) => (
          <ViewType key={t} id={t} icon={icons[t]} tabbar="auto" scaling={false}>
            <ViewBody render={render} />
          </ViewType>
        ))}
      </Workspace>
    </>
  )
}

/** The tab bar of the pane at the window's top-left corner (`first`, shown while it holds several views) starts right
 * of the macOS traffic lights while the sidebar is closed, as that pane's header does. The rule keys off the sidebar's
 * own `data-state` / `data-collapsible`, so a toggle swaps no stylesheet (and this never re-renders for one). */
function CornerTabs() {
  const first = usePanes((s) => s.first)
  if (first === null || !isMacDesktop()) return null
  const bar = `[data-slot='sidebar-inset'] .trellis [data-trellis-part='tabbar']:has([data-trellis-part='tab'][data-view=${JSON.stringify(first)}])`
  const closed = `[data-slot='sidebar'][data-state='collapsed']`
  // Beside the collapsed rail only the lights' overhang past its 3.5rem is left to clear (`cornerPad`).
  return <style>{`${closed} ~ ${bar} { padding-inline-start: 5rem; } ${closed}[data-collapsible='icon'] ~ ${bar} { padding-inline-start: 1.5rem; }`}</style>
}

/**
 * The chat area. Tiling (768 px and up): the Trellis workspace (`Tiled`) — splits, dividers, drag and drop and tabs
 * are Trellis's; every leaf kind (chat, draft, web frame, each tile kind in `kinds`) is one of its view types. A phone
 * (`isMobile`) does not tile (25 §12.3): the focused chat alone fills the screen as `first`, so its header always
 * carries the sidebar toggle — the Sheet's only opener. Crossing into the phone layout closes the generic tiles (their
 * surfaces open as sheets there) and puts the web tiles into the pill; a phone shows a web page full screen over the
 * chat (`PhoneWebView`, 209 §7), which becomes a tile beside its chat once the layout tiles again (`crossLayout`). That
 * page belongs to its chat: a route to another chat puts it into the pill, and while it shows the chat under it is
 * `inert`.
 */
export function Panes({ session, kinds, icons = NO_ICONS, render }: { session: string; kinds: readonly string[]; icons?: Icons; render: Render }) {
  const isMobile = useSidebarIsMobile()
  const leaves = usePanes((s) => s.leaves)
  const focused = usePanes((s) => s.focused)
  const attached = usePanes((s) => s.ws !== null)
  const covered = useWebViews((s) => isMobile && s.front !== null && s.views[s.front] !== undefined)
  const navigate = useNavigate()
  // The route's chat at the crossing, not on every navigation: `session` is read, not a dependency.
  useEffect(() => {
    if (!isMobile) return
    // A talk running in its pane keeps running: its view becomes the page, as the pane goes with the other tiles.
    const t = talk.currentTalk()
    const talking = talkPaneToKeep(t.phase === 'off' ? null : t.session)
    closeAllTiles()
    crossLayout(true, session)
    if (talking) void navigate({ to: '/talk/$session', params: { session: talking } })
  }, [isMobile])
  // Back to tiling: the page shown full screen becomes its tile, once the workspace is there to take it.
  useEffect(() => {
    if (attached) crossLayout(false, session)
  }, [attached])
  // On mount too: a page left full screen when the panes unmounted (Settings) must not reappear over another chat.
  useEffect(() => {
    const { front, views, minimise } = useWebViews.getState()
    const view = front === null ? undefined : views[front]
    if (view && view.session !== session) minimise(view.id)
  }, [session])
  if (!isMobile) {
    return (
      <div className="relative min-h-0 min-w-0 flex-1">
        <Tiled session={session} kinds={kinds} icons={icons} render={render} />
      </div>
    )
  }
  const leaf = shown(leaves, focused, session)
  return (
    <div className="relative min-h-0 min-w-0 flex-1">
      <div key={leaf} data-pane={leaf} inert={covered} className="absolute inset-0 flex min-h-0 min-w-0 flex-col">
        {render(leaf, true)}
      </div>
      <PhoneWebView />
    </div>
  )
}
