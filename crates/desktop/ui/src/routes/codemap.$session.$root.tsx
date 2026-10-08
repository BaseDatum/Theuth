import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { type PanelRoute, POPOUT_STATE_KEY, PoppedCodeMap } from '@/components/codemap/CodeMapPanel'
import { decodeMapSearch, encodeMapSearch, type Tab } from '@/lib/codemap/map'

type Search = { tab?: Tab; el?: string; sel?: string }

/**
 * One session's CodeMap on one root, full window (`lib/popout.ts` kind `codemap`, 73 §6.9). Under the `Popped` root when
 * opened with `?window=`. 147 §11.5: `?tab=map&el=<element>[&sel=<child>]` is the deep link; a drill pushes a history
 * entry, so browser Back = Esc. *Open in new window* hands the docked pane's `{ tab, nav }` over through
 * `localStorage` (`POPOUT_STATE_KEY`) because `popout.ts` builds the URL without search params; consumed once.
 */
export const Route = createFileRoute('/codemap/$session/$root')({
  validateSearch: (s: Record<string, unknown>): Search => {
    const { tab, el } = decodeMapSearch(s)
    const out: Search = encodeMapSearch({ tab, el })
    if (typeof s.sel === 'string' && s.sel) out.sel = s.sel
    return out
  },
  component: CodemapRoute,
})

function CodemapRoute() {
  const { session, root } = Route.useParams()
  const search = Route.useSearch()
  const navigate = useNavigate()
  const { tab, el } = decodeMapSearch(search)
  const sel = search.sel ?? null
  const go = (next: Search, push: boolean) => void navigate({ to: '/codemap/$session/$root', params: { session, root }, search: next, replace: !push })
  // the handoff from the docked pane, once, only when the URL carries no state of its own
  useEffect(() => {
    const raw = localStorage.getItem(POPOUT_STATE_KEY)
    if (!raw) return
    localStorage.removeItem(POPOUT_STATE_KEY)
    if (search.tab || search.el) return
    try {
      const s = JSON.parse(raw) as { key: string; tab: Tab; nav: { view: string | null; selected: string | null } }
      if (s.key !== `${session}:${root}`) return
      const next: Search = encodeMapSearch({ tab: s.tab, el: s.nav.view })
      if (s.nav.selected && s.nav.selected !== s.nav.view) next.sel = s.nav.selected
      go(next, false)
    } catch {
      /* a stale handoff is ignored */
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per mount
  }, [])
  const route: PanelRoute = {
    tab,
    nav: { view: el, selected: sel ?? el },
    onRoute: (t, nav, push) => {
      const next: Search = encodeMapSearch({ tab: t, el: nav.view })
      if (nav.selected && nav.selected !== nav.view) next.sel = nav.selected
      go(next, push)
    },
  }
  return <PoppedCodeMap session={session} root={root} route={route} />
}
