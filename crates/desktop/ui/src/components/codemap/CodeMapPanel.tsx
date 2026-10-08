/**
 * 73 §6 + 147 §11 — the CodeMap panel body: status header · tabs `Map | Here | Fit | Change` (Map default, pinned; 147
 * O9) · one body (the Map tab or a lens) · activity trail · detail line. Lenses are agent-driven with a user pin (73
 * §6.1); on Map a `CodeToolRan` only dots the lens it would show (147 §11.7). Rows come from `codemap_*` RPCs (mock or
 * engine, `lib/codemap/api.ts`, `lib/codemap/map-adapter.ts`), events from the session slice (`CodeToolRan`) and
 * `on('CodeReconciled')`; live edits reach the engine through `codemap_touch` (500 ms coalesced, §0.7). Nothing here
 * walks a tree or lays out more than the visible set.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { type CSSProperties, type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { appendDraft, prefillDraft } from '@/components/chat/Composer'
import { DiagramFrame } from '@/components/chat/DiagramFrame'
import { Button } from '@/components/ui/button'
import { useIsMobile } from '@/hooks/use-mobile'
import { codemapMock, disconnectedCodemap, mockCodemap } from '@/lib/codemap/api'
import { toGraphFence } from '@/lib/codemap/fence'
import { anchorOf, basename } from '@/lib/codemap/labels'
import { type Lens, type LensState, follow, initialLens, onCodeToolRan, pin, replay } from '@/lib/codemap/lens'
import { type Tab, TABS } from '@/lib/codemap/map'
import { engineMapApi, type MapApi, mapMethodsOf, mockMapApi } from '@/lib/codemap/map-adapter'
import type { Slots } from '@/lib/codemap/layout'
import type { ChangeLens, CodeMapChanged, CodeMapProgress, CodemapApi, FitLens, HereLens, MapRow } from '@/lib/codemap/types'
import { diagramTokens } from '@/lib/diagram-theme'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { createRenderQueue, type RenderQueue } from '@/lib/render-queue'
import { subscribeDark } from '@/lib/theme'
import { cn } from '@/lib/utils'
import { useConnection } from '@/store/connection'
import { dispatch, on, useSessionField, useSessions } from '@/store/sessions'
import { Change, ChangeList } from './Change'
import { Fit, FitList } from './Fit'
import { Here } from './Here'
import { type MapNav, MapTab } from './Map'

const LENSES: Lens[] = ['here', 'fit', 'change']
/** 147 §11.5 / 73 §4: `{ tab, element }` per root, per window, never persisted. */
const MEMORY = new Map<string, { tab: Tab; nav: MapNav }>()
/** The pop-out handoff of the current `{ tab, nav }` (`lib/popout.ts` builds the URL without search params). */
export const POPOUT_STATE_KEY = 'codemap-popout-state'
const REFETCH_MS = 200
const TOUCH_MS = 500
/** §6.9: list forms under this panel width; the trail expands and Fit gets its context column above 1080. */
const GRAPH_MIN_W = 600
const WIDE_W = 1080

type Data = { here?: HereLens; fit?: FitLens; change?: ChangeLens }
type Target = { path: string; a?: number; b?: number; moniker?: string; name?: string }

const lensKey = (s: LensState) =>
  s.lens === 'here'
    ? `here:${(s.params.here.focus ?? []).join(',')}`
    : s.lens === 'fit'
      ? `fit:${s.params.fit?.symbol ?? ''}`
      : `change:${s.params.change.git_range ?? ''}:${(s.params.change.files ?? []).join(',')}:${s.params.change.include_tests}`

function useCodemapApi(): CodemapApi {
  const api = useConnection((s) => s.api)
  return useMemo(() => (codemapMock() ? mockCodemap() : (api ?? disconnectedCodemap)), [api])
}

let mockMap: MapApi | null = null
/** The map adapter: the mock (events replayed into the store) under the dev flag, else the engine's methods. */
function useMapApi(): { api: MapApi; has: ReadonlySet<keyof MapApi> } {
  const api = useConnection((s) => s.api)
  return useMemo(() => {
    if (codemapMock()) {
      mockMap ??= mockMapApi((e: CodeMapProgress | CodeMapChanged) => dispatch([e as never]))
      return { api: mockMap, has: new Set(['codemap_overview', 'codemap_element', 'codemap_generate', 'codemap_cancel', 'codemap_search'] as const) }
    }
    return { api: engineMapApi(api), has: mapMethodsOf(api) }
  }, [api])
}

function useWidth(ref: React.RefObject<HTMLElement | null>): number {
  const [w, setW] = useState(0)
  useEffect(() => {
    const el = ref.current
    if (!el) return
    setW(el.clientWidth)
    const ro = new ResizeObserver(() => setW(el.clientWidth))
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref])
  return w
}

const hhmm = (at: number) => new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })

export type PanelRoute = { tab: Tab; nav: MapNav; onRoute(tab: Tab, nav: MapNav, push: boolean): void }

export function CodeMapPanel({ session, root, onClose, popped = false, route }: { session: string; root: string; onClose?(): void; popped?: boolean; route?: PanelRoute }) {
  const api = useCodemapApi()
  const map = useMapApi()
  const qc = useQueryClient()
  const engine = useConnection((s) => s.api)
  const codemap = useSessionField(session, 'codemap')
  const mobile = useIsMobile()
  const bodyRef = useRef<HTMLDivElement>(null)
  const width = useWidth(bodyRef)
  const graphs = !mobile && width >= GRAPH_MIN_W
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'))
  const tokens = useMemo(diagramTokens, [dark])

  // §12 Q4: a history-loaded `"?"` root resolves to the engine's default root through `codeindex_stats(null)`.
  const stats = useQuery({
    queryKey: ['codemap-stats', root],
    queryFn: () => engine!.codeindex_stats(root === '?' ? null : root),
    enabled: engine !== null,
    staleTime: Infinity,
  })
  const realRoot = root === '?' ? (stats.data?.root ?? null) : root

  // ── tabs (147 O9): Map first and default; `{ tab, nav }` per root per window (MEMORY), or the pop-out's URL (`route`) ──
  const memKey = `${session}:${root}`
  const [local, setLocal] = useState<{ tab: Tab; nav: MapNav }>(() => MEMORY.get(memKey) ?? { tab: 'map', nav: { view: null, selected: null } })
  const tab = route ? route.tab : local.tab
  const nav = route ? route.nav : local.nav
  const setRoute = (next: { tab: Tab; nav: MapNav }, push = false) => {
    if (route) route.onRoute(next.tab, next.nav, push)
    else {
      MEMORY.set(memKey, next)
      setLocal(next)
    }
  }
  const setTab = (t: Tab) => setRoute({ tab: t, nav })
  /** seq of a `CodeToolRan` that arrived while Map was shown and no lens tab has been opened since → the auto-dot (§11.7). */
  const [agentSeq, setAgentSeq] = useState<number | null>(null)
  const tabRef = useRef(tab)
  tabRef.current = tab
  // ── lens state machine (§6.1); seeded by replaying this root's trail oldest→newest, so a panel opened late shows what the
  // agent looked at and every lens already has the params of its last call ──
  const [lens, setLens] = useState<LensState>(() =>
    (codemap?.trail ?? [])
      .filter((t) => t.root === root)
      .reverse()
      .reduce((s, t) => onCodeToolRan(s, { type: 'CodeToolRan', session, root, call_id: t.callId, tool: t.tool, args: t.args, summary: t.summary, seq: t.seq }), initialLens()),
  )
  const lastSeq = codemap?.last?.seq ?? -1
  const seenSeq = useRef(lastSeq)
  useEffect(() => {
    const l = codemap?.last
    if (!l || l.seq === seenSeq.current || l.root !== realRoot) return
    seenSeq.current = l.seq
    if (tabRef.current === 'map') setAgentSeq(l.seq)
    setLens((s) => onCodeToolRan(s, { type: 'CodeToolRan', session, root: l.root, call_id: l.callId, tool: l.tool, args: l.args as Record<string, unknown>, summary: l.summary, seq: l.seq }))
  }, [lastSeq, codemap, realRoot, session])

  // ── data: one queue per panel, ≥ 200 ms between fetches; replies older than the shown seq are dropped (§8) ──
  const [data, setData] = useState<Data>({})
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const shownSeq = useRef(0)
  const queue = useRef<RenderQueue<LensState> | null>(null)
  const edited = codemap?.edited[realRoot ?? ''] ?? []
  const editedRef = useRef(edited)
  editedRef.current = edited
  useEffect(() => {
    if (!realRoot) return
    const q = createRenderQueue<LensState>(async (s, isCurrent) => {
      setBusy(true)
      try {
        const limitFit = popped ? 40 : 20
        const reply =
          s.lens === 'here'
            ? { here: await api.codemap_here(realRoot, s.params.here.focus, editedRef.current.length ? editedRef.current : null, 2000) }
            : s.lens === 'fit'
              ? s.params.fit
                ? { fit: await api.codemap_fit(realRoot, s.params.fit.symbol, limitFit) }
                : {}
              : { change: await api.codemap_change(realRoot, s.params.change.git_range, s.params.change.files, s.params.change.include_tests) }
        const meta = Object.values(reply)[0]?.meta
        if (!isCurrent() || (meta && meta.seq < shownSeq.current)) return
        if (meta) shownSeq.current = meta.seq
        setData((d) => ({ ...d, ...reply }))
        setError(null)
      } catch (e) {
        if (isCurrent()) setError(e instanceof Error ? e.message : String(e))
      } finally {
        if (isCurrent()) setBusy(false)
      }
    }, REFETCH_MS)
    queue.current = q
    return () => q.dispose()
  }, [api, realRoot, popped])
  const key = lensKey(lens)
  const meta = data[lens.lens]?.meta ?? data.here?.meta ?? data.fit?.meta ?? data.change?.meta
  const frozen = meta?.suspect_root === true
  useEffect(() => {
    if (!frozen && tab !== 'map') queue.current?.request(lens)
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `key` names every param the fetch reads
  }, [key, realRoot, frozen, tab])

  // ── slots reset on a new picture (§6.6 rule 6): a new Fit focus or Change mode/range ──
  const slots = useMemo<Slots>(() => new Map(), [key])

  // ── CodeReconciled (global, by root): refetch when it touched what is on screen; stats invalidated ──
  const visiblePaths = useMemo(() => {
    const set = new Set<string>()
    if (lens.lens === 'here') for (const r of data.here?.rows ?? []) set.add(r.path)
    if (lens.lens === 'fit' && data.fit) for (const n of [data.fit.focus, ...data.fit.callers, ...data.fit.callees]) set.add(n.path)
    if (lens.lens === 'change' && data.change) for (const f of [...data.change.changed, ...data.change.direct, ...data.change.transitive, ...data.change.tests]) set.add(f.path)
    return set
  }, [lens.lens, data])
  useEffect(
    () =>
      on('CodeReconciled', (e) => {
        if (e.root !== realRoot) return
        void qc.invalidateQueries({ queryKey: ['codemap-stats', root] })
        const hit = e.reparsed.some((p) => visiblePaths.has(p)) || e.removed.some((p) => visiblePaths.has(p))
        if (hit || (lens.lens === 'change' && lens.params.change.git_range === null && lens.params.change.files === null)) queue.current?.request(lens)
      }),
    [realRoot, root, visiblePaths, lens, qc],
  )

  // ── live edits → codemap_touch, coalesced 500 ms; only a mounted panel pays (§0.7); never under suspect_root ──
  const touched = useRef(0)
  useEffect(() => {
    if (!realRoot || frozen || edited.length <= touched.current) return
    const t = setTimeout(() => {
      const paths = edited.slice(touched.current)
      touched.current = edited.length
      api.codemap_touch(realRoot, paths).catch(() => {})
    }, TOUCH_MS)
    return () => clearTimeout(t)
  }, [edited, realRoot, frozen, api])

  // ── selection, hover, detail line, action menu ──
  const [selected, setSelected] = useState<string | null>(null)
  const [hover, setHover] = useState<string | null>(null)
  const [menu, setMenu] = useState<{ target: Target; x: number; y: number } | null>(null)
  const [snap, setSnap] = useState(false)
  const [trailOpen, setTrailOpen] = useState(false)
  const showTrail = trailOpen || width >= WIDE_W
  const detail = detailFor(lens.lens, data, hover ?? selected)

  const targetOf = (id: string): Target | null => {
    if (id.startsWith('bundle:')) return null
    if (lens.lens === 'here') {
      const r = data.here?.rows.find((r) => r.moniker === id)
      return r ? { path: r.path, a: r.line, b: r.line_end, moniker: r.moniker, name: r.name } : null
    }
    if (lens.lens === 'fit' && data.fit) {
      const n = [data.fit.focus, ...data.fit.callers, ...data.fit.callees].find((n) => n.moniker === id)
      if (n) return { path: n.path, a: n.line_start, b: n.line_end, moniker: n.moniker, name: n.name }
      const path = id.slice(id.indexOf(':') + 1)
      return { path }
    }
    return { path: id }
  }
  const openMenu = (id: string, at: { x: number; y: number }) => {
    const target = targetOf(id)
    if (target) setMenu({ target, x: at.x, y: at.y })
  }
  const select = (id: string) => {
    setSelected(id)
    if (id === 'bundle:0') setExpand('callers')
    else if (id === 'bundle:2') setExpand('callees')
  }
  const [expand, setExpand] = useState<'callers' | 'callees' | number | null>(null)
  const act = (what: 'ask' | 'fit' | 'change') => {
    const t = menu?.target
    setMenu(null)
    if (!t || !realRoot) return
    if (what === 'ask') {
      appendDraft(session, `\`${anchorOf(t.path, t.a, t.b)}\` `, `[codemap ${basename(realRoot)} · ${lens.lens}${lens.focus ? ` · ${lens.focus}` : ''}]\n`)
      window.dispatchEvent(new Event('theuth:focus-composer'))
    } else if (what === 'fit') setLens((s) => pin(s, 'fit', { fit: { symbol: t.moniker ?? t.name ?? t.path } }, t.moniker ?? null))
    else setLens((s) => pin(s, 'change', { change: { git_range: null, files: [t.path], include_tests: true } }))
  }
  const snapshot = (how: 'copy' | 'ask') => {
    setSnap(false)
    const cur = data[lens.lens]
    if (!cur || !realRoot) return
    const s = lens.lens === 'here' ? { lens: 'here' as const, data: cur as HereLens } : lens.lens === 'fit' ? { lens: 'fit' as const, data: cur as FitLens } : { lens: 'change' as const, data: cur as ChangeLens }
    const fence = toGraphFence(s)
    if (how === 'copy') {
      void navigator.clipboard.writeText(fence).then(() => toast.success('Copied a Graph fence for the board'))
    } else {
      prefillDraft(session, `[codemap ${basename(realRoot)} · ${lens.lens}${lens.focus ? ` · ${lens.focus}` : ''}]\n${fence}\nAdd this to the board's Map section.`)
      window.dispatchEvent(new Event('theuth:focus-composer'))
    }
  }
  const reconcile = () => realRoot && api.codemap_touch(realRoot, null).then(() => queue.current?.request(lens), (e) => toast.error(String(e)))

  const keys = (e: KeyboardEvent<HTMLDivElement>) => {
    if ((e.target as HTMLElement).tagName === 'INPUT') return
    if (e.key === 'Escape') {
      if (menu || snap) {
        setMenu(null)
        setSnap(false)
      } else onClose?.()
    } else if (e.key === '1') setTab('map')
    else if (e.key === '2' || e.key === '3' || e.key === '4') showLens(LENSES[Number(e.key) - 2]!)
    else if (e.key === 'p') {
      if (tab === 'map' || lens.mode === 'pinned') followAgent()
      else setLens((s) => pin(s, s.lens))
    } else return
    e.stopPropagation()
  }
  /** Open a lens tab pinned (73 §6.1); the auto-dot clears because the user looked. */
  const showLens = (l: Lens) => {
    setAgentSeq(null)
    setTab(l)
    setLens((s) => pin(s, l))
  }
  /** *Follow agent*: the lens the last call would show, in `auto` (§11.7). */
  const followAgent = () => {
    setAgentSeq(null)
    setTab(lens.auto)
    setLens(follow)
  }
  const openPopped = () => {
    if (!realRoot) return
    localStorage.setItem(POPOUT_STATE_KEY, JSON.stringify({ key: `${session}:${realRoot}`, tab, nav }))
    void openPopout({ kind: 'codemap', session, root: realRoot })
  }

  useEffect(() => {
    if (popped && realRoot) document.title = `CodeMap · ${basename(realRoot)} · ${tab}`
  }, [popped, realRoot, tab])

  // ── render ──
  const trail = (codemap?.trail ?? []).filter((t) => t.root === realRoot)
  const calls = trail.length
  const fresh = !meta ? null : meta.stale ? `stale · ${meta.pending} pending` : 'fresh'
  const hereFocus = lens.params.here.focus ?? []
  const marker = trail[0]?.target ?? null
  const stale = useMemo(() => new Set<string>(data.change?.unindexed ?? []), [data.change])

  let body: ReactNode
  if (tab === 'map')
    body = realRoot ? (
      <MapTab session={session} root={realRoot} api={map.api} has={map.has} live={engine !== null} width={width} mobile={mobile} popped={popped} tokens={tokens} trail={trail} edited={edited} nav={nav} onNav={(n, push) => setRoute({ tab: 'map', nav: n }, push)} />
    ) : (
      <p className="p-3 font-mono text-[11px] text-fg-secondary">resolving root…</p>
    )
  else if (error) body = <p className="p-3 font-mono text-[11px] text-fg-secondary">{/not indexed/i.test(error) ? 'run a code_* tool first' : error}</p>
  else if (lens.lens === 'here' && data.here)
    body = <Here here={data.here} focus={hereFocus} edited={edited} marker={marker} stale={!!meta?.stale} dim={frozen} onHover={(r) => setHover(r?.moniker ?? null)} onSelect={(r) => select(r.moniker)} onContext={(r, at) => openMenu(r.moniker, at)} />
  else if (lens.lens === 'fit' && data.fit) {
    const common = { fit: data.fit, popped, stale, selected, dim: frozen, onSelect: select, onHover: setHover, onContext: openMenu, onExpandBundle: (side: 'callers' | 'callees') => setExpand(side) }
    body = graphs && expand === null ? (
      <DiagramFrame name={`codemap-fit-${basename(data.fit.focus.path)}`} label={`fit · ${data.fit.focus.name}`} kind="codemap-fit" fill>
        <Fit {...common} width={width - 24} tokens={tokens} slots={slots} />
      </DiagramFrame>
    ) : (
      <FitList {...common} />
    )
  } else if (lens.lens === 'change' && data.change) {
    const common = { change: data.change, popped, selected, dim: frozen, onSelect: select, onHover: setHover, onContext: openMenu, onExpandBundle: (layer: number) => setExpand(layer) }
    body = graphs && expand === null ? (
      <DiagramFrame name={`codemap-change-${data.change.mode}`} label={`change · ${data.change.mode}`} kind="codemap-change" fill>
        <Change {...common} width={width - 24} tokens={tokens} slots={slots} />
      </DiagramFrame>
    ) : (
      <ChangeList {...common} />
    )
  } else if (lens.lens === 'fit' && !lens.params.fit) body = <p className="p-3 font-mono text-[11px] text-fg-secondary">no symbol yet — run code_refs or code_symbol, or pick “How it fits” on a row</p>
  else body = <p className="p-3 font-mono text-[11px] text-fg-secondary">{busy ? 'loading…' : realRoot ? '' : 'resolving root…'}</p>

  return (
    <div className="flex h-full min-h-0 flex-col font-mono text-[11px]" onKeyDown={keys} data-codemap-panel data-tab={tab} data-lens={lens.lens} data-mode={lens.mode}>
      <header className="flex h-8 shrink-0 items-center gap-2 overflow-hidden border-b border-border-subtle px-2 whitespace-nowrap">
        <span className="font-semibold">CodeMap</span>
        <RootPicker session={session} root={root} roots={codemap?.roots ?? []} popped={popped} />
        {/* §6.9 phones: the lens tabs and actions win the row; counts return above 520 px */}
        {width >= 520 && stats.data && (
          <span className="text-fg-secondary">
            {Number(stats.data.files).toLocaleString()} files · {Number(stats.data.symbols).toLocaleString()} symbols
          </span>
        )}
        {fresh && <Chip>{fresh}</Chip>}
        {meta?.truncated_walk && <Chip>walk capped</Chip>}
        {busy && tab === 'change' && data.change && <Chip>computing…</Chip>}
        {width >= 520 && <span className="text-fg-secondary">{calls} calls</span>}
        <nav className="ml-auto flex items-center gap-1" aria-label="Lens">
          {TABS.map((t) => {
            // 147 §11.7: on Map the lens a call would show gets the dot; on a lens tab 73's pinned rule stands
            const dot = t !== 'map' && (tab === 'map' ? agentSeq !== null && lens.auto === t : lens.mode === 'pinned' && lens.auto === t && lens.lens !== t)
            return (
              <button
                key={t}
                type="button"
                aria-pressed={tab === t}
                onClick={() => (t === 'map' ? setTab('map') : showLens(t))}
                className={cn('relative rounded px-1.5 py-0.5 hover:bg-layer-2', tab === t && 'bg-layer-3 font-semibold')}
                title={`${t} (${TABS.indexOf(t) + 1})`}
              >
                {t}
                {dot && <span aria-label="agent is here" className="ml-0.5 inline-block size-1.5 rounded-full bg-fg-accent align-middle" />}
              </button>
            )
          })}
          {(tab === 'map' ? agentSeq !== null : lens.mode === 'pinned') && (
            <button type="button" className="rounded px-1.5 py-0.5 text-fg-secondary hover:bg-layer-2" onClick={followAgent} title="Follow the agent (p)">
              follow
            </button>
          )}
          {tab !== 'map' && (
            <button type="button" className="rounded px-1.5 py-0.5 hover:bg-layer-2" title="Snapshot to board" aria-label="Snapshot to board" onClick={() => setSnap((v) => !v)}>
              ⊡
            </button>
          )}
          {!popped && !mobile && !isPopout() && canPopout() && realRoot && (
            <button type="button" className="rounded px-1.5 py-0.5 hover:bg-layer-2" title="Open in new window" aria-label="Open in new window" onClick={openPopped}>
              ⤢
            </button>
          )}
          {onClose && (
            <button type="button" className="rounded px-1.5 py-0.5 hover:bg-layer-2" aria-label="Close CodeMap" onClick={onClose}>
              ✕
            </button>
          )}
        </nav>
      </header>
      {frozen && (
        <div role="alert" className="flex shrink-0 items-center gap-2 border-b border-border-subtle bg-layer-2 px-2 py-1">
          <span className="flex-1">{meta?.hint ?? 'suspect root'}</span>
          <Button size="sm" variant="outline" onClick={reconcile}>
            Reconcile
          </Button>
        </div>
      )}
      {tab === 'fit' && data.fit && data.fit.candidates.length > 0 && <div className="shrink-0 px-2 py-0.5 text-fg-secondary">1 of {data.fit.candidates.length} candidates</div>}
      {tab !== 'map' && expand !== null && (
        <button type="button" className="shrink-0 px-2 py-0.5 text-left text-fg-secondary hover:bg-layer-2" onClick={() => setExpand(null)}>
          ← back to the picture
        </button>
      )}
      <div ref={bodyRef} className="relative min-h-0 flex-1 overflow-auto" onPointerLeave={() => setHover(null)}>
        {body}
        {menu && (
          <ActionMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)}>
            <MenuItem onClick={() => act('ask')}>Ask about this</MenuItem>
            <MenuItem onClick={() => act('fit')}>How it fits</MenuItem>
            <MenuItem onClick={() => act('change')}>What changes</MenuItem>
          </ActionMenu>
        )}
        {snap && (
          <ActionMenu x={width - 8} y={4} right onClose={() => setSnap(false)}>
            <MenuItem onClick={() => snapshot('copy')}>Copy fence</MenuItem>
            <MenuItem onClick={() => snapshot('ask')}>Ask to add to board</MenuItem>
          </ActionMenu>
        )}
      </div>
      {trail.length > 0 && (
        <section className="shrink-0 border-t border-border-subtle">
          <button type="button" className="flex w-full items-center gap-2 px-2 py-0.5 text-left text-fg-secondary hover:bg-layer-2" aria-expanded={showTrail} onClick={() => setTrailOpen((v) => !v)}>
            <span>{showTrail ? '▾' : '▸'} trail</span>
            {!showTrail && trail[0] && (
              <span className="truncate">
                {hhmm(trail[0].at)} {trail[0].tool} {trail[0].target ?? ''}
              </span>
            )}
          </button>
          {showTrail && (
            <ol className="max-h-28 overflow-auto px-2 pb-1">
              {trail.map((t) => (
                <li key={t.callId}>
                  <button
                    type="button"
                    className={cn('w-full truncate text-left hover:bg-layer-2', t.callId === lens.callId && 'underline')}
                    onClick={() => {
                      setAgentSeq(null)
                      const next = replay(lens, t)
                      setTab(next.lens)
                      setLens(next)
                    }}
                    title={`replay ${t.tool} as pinned`}
                  >
                    {hhmm(t.at)} {t.tool} {t.target ?? ''}
                  </button>
                </li>
              ))}
            </ol>
          )}
        </section>
      )}
      <footer className="h-6 shrink-0 truncate border-t border-border-subtle px-2 leading-6 text-fg-secondary" aria-live="polite">
        {detail}
      </footer>
    </div>
  )
}

/** §6.5: `path:a-b · kind · name · #rank · →N ↑N · provenance confidence` — the one place rank and confidence numbers appear. */
function detailFor(lens: Lens, data: Data, id: string | null): string {
  if (!id) return ''
  if (lens === 'here') {
    const rows = data.here?.rows ?? []
    const r = rows.find((r) => r.moniker === id)
    if (!r) return ''
    const rank = rows.filter((x) => x.rank > r.rank).length + 1
    return [`${r.path}:${r.line}-${r.line_end}`, r.kind, r.name, `#${rank}`, badges(r), r.signature].filter(Boolean).join(' · ')
  }
  if (lens === 'fit' && data.fit) {
    const f = data.fit
    const n = [f.focus, ...f.callers, ...f.callees].find((n) => n.moniker === id)
    if (!n) return id.startsWith('bundle:') ? `${id === 'bundle:0' ? f.more_callers : f.more_callees} more — tap to list` : id.slice(id.indexOf(':') + 1)
    const e = f.edges.find((e) => (e.from === id && e.to === f.focus.moniker) || (e.to === id && e.from === f.focus.moniker))
    return [`${n.path}:${n.line_start}-${n.line_end}`, n.kind, n.name, n.moniker === f.focus.moniker && f.ambiguous ? `?${f.ambiguous} ambiguous` : '', e ? `${e.provenance} ${e.confidence.toFixed(1)}${e.count > 1 ? ` ×${e.count}` : ''}` : ''].filter(Boolean).join(' · ')
  }
  if (lens === 'change' && data.change) {
    const c = data.change
    const f = [...c.changed, ...c.direct, ...c.transitive, ...c.tests].find((f) => f.path === id)
    if (!f) return ''
    return [f.path, f.status ?? '', f.via.length ? `via ${f.via.join(', ')}` : '', f.symbols.length ? f.symbols.map((s) => s.name).join(', ') : '', f.test_link ? `test ${f.test_link}` : '', f.depth ? `depth ${f.depth}` : ''].filter(Boolean).join(' · ')
  }
  return ''
}

const badges = (r: MapRow) => [r.importers ? `→${r.importers}` : '', r.callers ? `↑${r.callers}` : ''].filter(Boolean).join(' ')

function Chip({ children }: { children: ReactNode }) {
  return <span className="rounded border border-border-subtle px-1 text-fg-secondary">{children}</span>
}

/** Root name; a `▾` menu when the session touched more than one root (§6.7). */
function RootPicker({ session, root, roots, popped }: { session: string; root: string; roots: string[]; popped: boolean }) {
  const [open, setOpen] = useState(false)
  const label = root === '?' ? 'default root' : basename(root)
  if (roots.length < 2 || popped) return <span title={root}>{label}</span>
  return (
    <span className="relative">
      <button type="button" className="hover:underline" onClick={() => setOpen((v) => !v)} title={root} aria-haspopup="menu" aria-expanded={open}>
        {label} ▾
      </button>
      {open && (
        <ActionMenu x={0} y={16} onClose={() => setOpen(false)}>
          {roots.map((r) => (
            <MenuItem
              key={r}
              onClick={() => {
                setOpen(false)
                useSessions.getState().setCodemapPanel(session, r)
              }}
            >
              {r === '?' ? 'default root' : basename(r)} <span className="text-fg-secondary">{r}</span>
            </MenuItem>
          ))}
        </ActionMenu>
      )}
    </span>
  )
}

/** A small anchored menu (right-click / `⋯` / Enter); Escape and outside pointer close it. No hover-only path: every item is a button. */
function ActionMenu({ x, y, right, onClose, children }: { x: number; y: number; right?: boolean; onClose(): void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[role=menuitem]')?.focus()
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose()
    }
    window.addEventListener('pointerdown', down, true)
    return () => window.removeEventListener('pointerdown', down, true)
  }, [onClose])
  const style: CSSProperties = right ? { right: 8, top: y } : { left: x, top: y }
  return (
    <div ref={ref} role="menu" className="absolute z-20 min-w-40 rounded-md border border-border-subtle bg-layer-1 p-1 shadow-md" style={style}>
      {children}
    </div>
  )
}

function MenuItem({ onClick, children }: { onClick(): void; children: ReactNode }) {
  return (
    <button type="button" role="menuitem" className="block w-full rounded px-2 py-1 text-left hover:bg-layer-2 focus:bg-layer-2 focus:outline-none" onClick={onClick}>
      {children}
    </button>
  )
}

/** The popped window (`routes/codemap.$session.$root.tsx`): the same body, full height, no close (§6.9); `route` carries `{ tab, element }` (147 §11.5). */
export function PoppedCodeMap({ session, root, route }: { session: string; root: string; route?: PanelRoute }) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <CodeMapPanel session={session} root={root} popped route={route} />
    </div>
  )
}
