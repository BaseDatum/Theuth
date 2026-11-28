import { RouterProvider, createMemoryHistory, createRouter, useNavigate } from '@tanstack/react-router'
import { useCallback, useEffect, useRef, useState } from 'react'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { ViewProps } from '@/components/tiles/open'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { usePanes } from '@/lib/panes'
import { deckPopoutTarget, openPopout } from '@/lib/popout'
import { setTileProps } from '@/lib/tiles'
import { normalizeViewPath, viewTitle } from '@/lib/view-title'
import { routeTree } from '@/routeTree.gen'

function isOuterRoute(href: string): { kind: 'chat'; session: string; href: string } | { kind: 'new'; href: string } | null {
  const path = normalizeViewPath(href)
  const chat = /^\/c\/([^/?#]+)/.exec(path)
  if (chat?.[1]) return { kind: 'chat', session: decodeURIComponent(chat[1]), href: path }
  if (path === '/' || /^\/new(?:$|[?#])/.test(path)) return { kind: 'new', href: path }
  return null
}

/**
 * Any-View tile (`tile:view`): renders any app route (Tasks, Notebooks, Notes, Inbox, Costs, Settings, Whiteboards, …)
 * inside a per-tile nested `<RouterProvider>` backed by memory history. `routes/__root.tsx` detects `useInsideTile()`
 * (provided by `TileFrame`) and renders only `<Outlet />` inside `<ResidentCtx value={false}>` so the tile never
 * duplicates the sidebar, status bar, or 52 px window header. Navigating to `/c/$session` or `/new` inside the tile is
 * intercepted and routed to the outer workspace instead of nesting a Trellis workspace inside a tile.
 */
function ViewTile({ tile, first, close }: TileRender) {
  const p = (tile.props ?? { path: '/' }) as ViewProps
  const initialPath = normalizeViewPath(p.path || '/')
  const outerNavigate = useNavigate()
  const outerNavigateRef = useRef(outerNavigate)
  outerNavigateRef.current = outerNavigate

  const propsRef = useRef(p)
  propsRef.current = p

  const lastPathRef = useRef(initialPath)
  const [currentPath, setCurrentPath] = useState(initialPath)
  // The tile's own history has a page before this one: its header shows Back.
  const [canBack, setCanBack] = useState(false)

  const interceptRef = useRef((href: string): boolean => {
    const outer = isOuterRoute(href)
    if (!outer) return false
    if (outer.kind === 'chat') {
      usePanes.getState().show(outer.session)
      void outerNavigateRef.current({ href: outer.href })
    } else {
      void outerNavigateRef.current({ href: outer.href })
    }
    return true
  })

  const [innerRouter] = useState(() => {
    const safeInitial = isOuterRoute(initialPath) ? '/tasks' : initialPath
    const history = createMemoryHistory({ initialEntries: [safeInitial] })
    const origPush = history.push.bind(history)
    const origReplace = history.replace.bind(history)
    history.push = (path, state, opts) => {
      if (interceptRef.current(path)) return
      origPush(path, state, opts)
    }
    history.replace = (path, state, opts) => {
      if (interceptRef.current(path)) return
      origReplace(path, state, opts)
    }
    return createRouter({
      routeTree,
      history,
      defaultPreload: false,
    })
  })

  // Sync inner navigations back to tile props so reload restores where the user navigated inside the tile.
  useEffect(() => {
    return innerRouter.subscribe('onResolved', ({ toLocation }) => {
      const nextHref = toLocation.href
      setCurrentPath(nextHref)
      setCanBack(innerRouter.history.canGoBack())
      if (nextHref !== lastPathRef.current) {
        const prev = propsRef.current
        const prevDefault = viewTitle(prev.path || '/')
        const keepTitle = prev.title && prev.title !== prevDefault && prev.path === nextHref ? prev.title : undefined
        lastPathRef.current = nextHref
        if (prev.path !== nextHref) {
          setTileProps(tile.id, keepTitle ? { path: nextHref, title: keepTitle } : { path: nextHref })
        }
      }
    })
  }, [innerRouter, tile.id])

  // Apply external prop updates (e.g. retargeting an open view tile).
  useEffect(() => {
    const next = normalizeViewPath(p.path || '/')
    if (next === lastPathRef.current) return
    if (interceptRef.current(next)) return
    lastPathRef.current = next
    setCurrentPath(next)
    if (innerRouter.state.location.href !== next) {
      void innerRouter.navigate({ href: next })
    }
  }, [innerRouter, p.path])

  const handleTileTitle = useCallback(
    (nextTitle: string) => {
      const trimmed = nextTitle.trim()
      if (!trimmed) return
      const prev = propsRef.current
      const path = lastPathRef.current
      if (prev.title === trimmed && normalizeViewPath(prev.path || '/') === path) return
      setTileProps(tile.id, { path, title: trimmed })
    },
    [tile.id],
  )

  const title = viewTitle(currentPath, p.title)

  return (
    <TileFrame
      first={first}
      onClose={close}
      onBack={canBack ? () => innerRouter.history.back() : undefined}
      onTitle={handleTileTitle}
      title={title}
      tab={title}
      popOut={() => {
        const deckTarget = deckPopoutTarget(currentPath, title !== 'Decks' ? title : undefined)
        return openPopout(deckTarget ?? { kind: 'page', path: currentPath })
      }}
      className="bg-background"
    >
      <RouterProvider router={innerRouter} />
    </TileFrame>
  )
}

export const viewTile: TileKind = {
  title: (tile) => {
    const p = (tile.props ?? { path: '/' }) as ViewProps
    return viewTitle(p.path || '/', p.title)
  },
  render: (p) => <ViewTile {...p} />,
}
