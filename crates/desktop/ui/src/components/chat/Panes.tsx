import { layout as L, useOptionalView, ViewType, Workspace, type MenuEntry, type WorkspaceHandle } from '@danfessler/trellis-react'
import '@danfessler/trellis/style.css'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useSidebar } from '@/components/ui/sidebar'
import { usePressDrag } from '@/hooks/use-press-drag'
import { TileHost } from '@/components/tiles/host'
import { useEngines } from '@/link/engines'
import {
  attachWorkspace,
  CHAT,
  DRAFT_TYPE,
  isDraft,
  isFrame,
  newDraft,
  picked,
  prepareStored,
  shown,
  STORAGE_KEY,
  tileType,
  usePanes,
  VERSION,
  WEB,
  zone,
} from '@/lib/panes'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { closeAllTiles } from '@/lib/tiles'
import { cn } from '@/lib/utils'
import { crossLayout } from '@/lib/web-arrival'
import { useWebViews } from '@/lib/web-views'
import { useQuerySupport } from '@/rpc/capabilities'
import { PhoneWebView } from '@/components/chat/PhoneWebView'

/**
 * Drag a sidebar row onto a chat pane (`usePressDrag` owns the gesture and its DOM glue). Spread the result on the
 * row's `Link`; a press that became a drag never navigates, selects or opens the row's context menu. `id === null`
 * is the New chat rail item: each drag carries a fresh draft id, and a drop opens a draft pane (`NewChatPane`)
 * without moving the URL. In the phone Sheet fingers never drag: phones do not tile.
 */
export function useSessionDrag(id: string | null, title: string) {
  const navigate = useNavigate()
  const { isMobile } = useSidebar()
  return usePressDrag({
    disabledForFinger: isMobile,
    onStart: (x, y) => usePanes.getState().startDrag(id ?? newDraft(), title, x, y),
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd(drop) {
      const dropped = usePanes.getState().endDrag(drop)
      if (dropped && !isDraft(dropped)) void navigate({ to: '/c/$session', params: { session: dropped } })
    },
  }).props
}

/** The drag ghost and the snapped drop zone; mounted once in the shell, renders nothing between drags. */
export function DragOverlay() {
  const drag = usePanes((s) => s.drag)
  if (!drag) return null
  return (
    <>
      {drag.target && (
        <div
          className="pointer-events-none fixed z-50 rounded-xl border-2 border-fg-accent bg-fg-accent/10 transition-[left,top,width,height] duration-100"
          style={zone(drag.target)}
        />
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
 * opens takes over the selected frame's pane. The subtree's surfaces open as tiles beside this view (`useTileHost`).
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

/** Built-in panel menu items that need what this app does not offer: floating, maximise, hiding (there is no dock). */
const UNUSED_MENU: Record<string, true> = { float: true, dock: true, maximize: true, hide: true }
const panelMenu = (entries: MenuEntry[]): MenuEntry[] => entries.filter((e) => e === 'separator' || !UNUSED_MENU[e.id ?? ''])

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
  useState(() => prepareStored({ engine, sessions: sessions && new Set(sessions), types: new Set(types) }))
  const [initial] = useState(() => L.view(CHAT, { id: session, params: { engine } }))
  const go = useRef(navigate)
  go.current = navigate
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
        navigation={false}
        panelMenu={panelMenu}
        storageKey={STORAGE_KEY}
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
 * of the macOS traffic lights while the sidebar is closed, as that pane's header does. */
function CornerTabs() {
  const { open } = useSidebar()
  const first = usePanes((s) => s.first)
  if (open || first === null || !isMacDesktop()) return null
  const tab = `[data-trellis-part='tab'][data-view='${CSS.escape(first)}']`
  return <style>{`.trellis [data-trellis-part='tabbar']:has(${tab}) { padding-inline-start: 5rem; }`}</style>
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
  const { isMobile } = useSidebar()
  const leaves = usePanes((s) => s.leaves)
  const focused = usePanes((s) => s.focused)
  const attached = usePanes((s) => s.ws !== null)
  const covered = useWebViews((s) => isMobile && s.front !== null && s.views[s.front] !== undefined)
  // The route's chat at the crossing, not on every navigation: `session` is read, not a dependency.
  useEffect(() => {
    if (!isMobile) return
    closeAllTiles()
    crossLayout(true, session)
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
