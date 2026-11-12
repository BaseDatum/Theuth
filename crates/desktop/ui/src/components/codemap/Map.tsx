import { useQuerySupport } from '@/rpc/capabilities'
/**
 * 147 §11 — the Map tab body: tree (left, 200 px, collapsible to a 28 px rail) · canvas (centre) · detail (right 280 px
 * at ≥ 1 080 px, else a drawer under the canvas); < 600 px panel width or a phone = `MapList`. Data: `codemap_overview`
 * on mount (first open in `Structure` → `codemap_generate(root, session, null)`, charged to this session, no
 * confirmation — S1/S2), `codemap_element` per shown element through react-query, `CodeMapProgress` → the progress
 * line, `CodeMapChanged` → refetch of the named elements (coalesced 200 ms). Navigation state `{ view, selected }` is
 * the caller's (the docked pane keeps it per root in memory; the pop-out keeps it in the URL, §11.5).
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { buildTreeRows, chainOf, defaultOpen, localSearch, type Mark, marksFor, parentsOf, progressLine } from '@/lib/codemap/map'
import type { MapApi } from '@/lib/codemap/map-adapter'
import type { CodeMapProgress, TrailRow } from '@/lib/codemap/types'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { cn } from '@/lib/utils'
import type { MapHit } from "@/rpc/bindings/method.codeindex.output"
import type { MapIndex } from "@/rpc/bindings/method.codeindex.output"
import type { MapJob } from "@/rpc/bindings/method.codeindex.output"
import { on } from '@/store/sessions'
import { MapCanvas } from './MapCanvas'
import { MapDetail } from './MapDetail'
import { MapList } from './MapList'
import { MapTree } from './MapTree'
import './codemap.css'

/** §6.9 / §11.1 thresholds: list form under 600 px; the detail pane docks right above 1 080 px. */
const LIST_W = 600
const WIDE_W = 1080
const REFETCH_MS = 200
/** The detail pane docks to the right of the canvas from here (the mock's 880 px pane: tree 180 · canvas · detail 260). */
const DOCK_W = 820
const SEARCH_MS = 150

export type MapNav = { view: string | null; selected: string | null }

export type MapProps = {
  session: string
  root: string
  api: MapApi
  has: ReadonlySet<keyof MapApi>
  /** A live engine connection (actions that write need one). */
  live: boolean
  width: number
  mobile: boolean
  popped: boolean
  tokens: DiagramTokens
  trail: readonly TrailRow[]
  edited: readonly string[]
  nav: MapNav
  /** `push` = a drill (a history entry in the pop-out); otherwise replace. */
  onNav(next: MapNav, push: boolean): void
}

const RUNNING = (stage: string | undefined) => !!stage && !['done', 'failed', 'cancelled'].includes(stage)



export function MapTab(p: MapProps) {
  const canQuery1 = useQuerySupport(["codemap_overview"])
  const canQuery1_2 = useQuerySupport(["codemap_element"])
  const canQuery1_3 = useQuerySupport(["codemap_element"])
  const { root, session, api, nav } = p
  const qc = useQueryClient()
  const index = useQuery({ enabled: canQuery1, queryKey: ['codemap-map', root], queryFn: () => api.codemap_overview(root), staleTime: 30_000, retry: false })
  const data: MapIndex | undefined = index.data
  const parents = useMemo(() => (data ? parentsOf(data) : new Map<string, string>()), [data])
  const viewId = nav.view ?? data?.perspectives[0]?.id ?? null
  const selectedId = nav.selected ?? viewId

  // ── elements shown: the canvas's and, when different, the detail's ──
  const view = useQuery({ queryKey: ['codemap-el', root, viewId], queryFn: () => api.codemap_element(root, viewId!), enabled: canQuery1_2 && (viewId !== null), staleTime: 30_000, retry: false })
  const sel = useQuery({ queryKey: ['codemap-el', root, selectedId], queryFn: () => api.codemap_element(root, selectedId!), enabled: canQuery1_3 && (selectedId !== null && selectedId !== viewId), staleTime: 30_000, retry: false })
  const detailEl = selectedId === viewId ? (view.data ?? null) : (sel.data ?? null)
  const detailErr = selectedId === viewId ? view.error : sel.error

  // ── generation: first open in Structure (or stale elements present) → generate, once per mount (§6.1) ──
  const [job, setJob] = useState<MapJob | null>(null)
  const [progress, setProgress] = useState<CodeMapProgress | null>(null)
  const [genError, setGenError] = useState<string | null>(null)
  const asked = useRef(false)
  const generate = useCallback(
    (element: string | null) => {
      setGenError(null)
      return api.codemap_generate(root, session, element).then(
        (j) => setJob(j),
        (e: unknown) => setGenError(e instanceof Error ? e.message : String(e)),
      )
    },
    [api, root, session],
  )
  useEffect(() => {
    const m = data?.meta
    if (!m || asked.current) return
    if (m.job && RUNNING(m.job.stage)) {
      setJob(m.job)
      asked.current = true
      return
    }
    if (m.state === 'structure' || (m.stale_elements > 0 && !RUNNING(m.job?.stage))) {
      asked.current = true
      void generate(null)
    }
  }, [data, generate])

  // ── events by root: progress line; changed → refetch named elements and the overview, coalesced ──
  const pending = useRef(new Set<string>())
  const flush = useRef(0)
  useEffect(() => {
    const offP = on('codeindex.events@1', 'CodeMapProgress', (e) => {
      if (e.root !== root) return
      setProgress(e)
      if (!RUNNING(e.stage)) {
        setJob(null)
        void qc.invalidateQueries({ queryKey: ['codemap-map', root] })
      }
    })
    const offC = on('codeindex.events@1', 'CodeMapChanged', (e) => {
      if (e.root !== root) return
      for (const id of [...e.elements, ...e.stale]) pending.current.add(id)
      if ((e.more ?? 0) > 0) pending.current.add('*')
      clearTimeout(flush.current)
      flush.current = window.setTimeout(() => {
        const ids = pending.current
        pending.current = new Set()
        void qc.invalidateQueries({ queryKey: ['codemap-map', root] })
        if (ids.has('*')) void qc.invalidateQueries({ queryKey: ['codemap-el', root] })
        else for (const id of ids) void qc.invalidateQueries({ queryKey: ['codemap-el', root, id] })
      }, REFETCH_MS)
    })
    return () => {
      offP()
      offC()
      clearTimeout(flush.current)
    }
  }, [root, qc])

  // ── tree state: folds, marks, search chain ──
  const [folds, setFolds] = useState<Map<string, boolean>>(() => new Map())
  const auto = useMemo(() => (data ? defaultOpen(data, selectedId) : new Set<string>()), [data, selectedId])
  const isOpen = useCallback((id: string) => folds.get(id) ?? auto.has(id), [folds, auto])
  const marks = useMemo<Map<string, Mark[]>>(
    () => (data ? marksFor(data, p.trail.map((t) => t.target), p.edited) : new Map()),
    [data, p.trail, p.edited],
  )
  const [query, setQuery] = useState('')
  const [hits, setHits] = useState<MapHit[]>([])
  const searchRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (!query.trim() || !data) {
      setHits([])
      return
    }
    let live = true
    const t = setTimeout(() => {
      api.codemap_search(root, query, 20).then(
        (h) => live && setHits(h),
        () => live && setHits(localSearch(data, query)),
      )
    }, SEARCH_MS)
    return () => {
      live = false
      clearTimeout(t)
    }
  }, [query, api, root, data])
  const chain = useMemo(() => chainOf(hits), [hits])
  const chainOpen = useCallback((id: string) => isOpen(id) || (chain.size > 0 && chain.has(id)), [isOpen, chain])
  const rows = useMemo(() => (data ? buildTreeRows(data, chainOpen, marks) : []), [data, chainOpen, marks])
  const status = useMemo(() => {
    const m = new Map<string, string>()
    if (!data || !RUNNING(progress?.stage ?? job?.stage)) return m
    const cur = progress?.element?.split('/')[0]
    for (const per of data.perspectives) {
      if (per.id === cur) m.set(per.id, 'generating…')
      else if (per.elements === 0) m.set(per.id, 'queued')
    }
    return m
  }, [data, progress, job])

  // ── navigation ──
  const go = (next: Partial<MapNav>, push = false) => p.onNav({ view: next.view === undefined ? viewId : next.view, selected: next.selected === undefined ? selectedId : next.selected }, push)
  const selectRow = (id: string) => {
    const parent = parents.get(id)
    go(parent ? { view: parent, selected: id } : { view: id, selected: id })
  }
  const drill = (id: string) => go({ view: id, selected: id }, true)
  const up = () => {
    if (!viewId) return
    const parent = parents.get(viewId)
    if (parent) go({ view: parent, selected: viewId }, true)
  }
  const regenerate = (id: string) => void generate(id).then(() => toast('Regenerating in this session’s cost'))

  const [railOnly, setRailOnly] = useState(false)
  const [detailOpen, setDetailOpen] = useState(true)
  const [sheet, setSheet] = useState<string | null>(null)
  const keys = (e: KeyboardEvent<HTMLDivElement>) => {
    const tag = (e.target as HTMLElement).tagName
    if (e.key === '/' && tag !== 'INPUT' && tag !== 'TEXTAREA') {
      e.preventDefault()
      e.stopPropagation()
      searchRef.current?.focus()
    } else if (e.key === 'Escape') {
      if (tag === 'INPUT' && query) {
        setQuery('')
      } else if (sheet) setSheet(null)
      else if (selectedId !== viewId) go({ selected: viewId })
      else if (viewId && parents.get(viewId)) up()
      else return
      e.stopPropagation()
    }
  }

  // ── banners ──
  const running = RUNNING(progress?.stage ?? job?.stage ?? data?.meta.job?.stage)
  const line = running ? progressLine(progress ?? job ?? data!.meta.job!) : progress && progress.stage === 'failed' ? progressLine(progress) : null
  const stale = data?.meta.stale_elements ?? 0
  const err = index.error instanceof Error ? index.error.message : index.error ? String(index.error) : null

  if (err) return <p className="p-3 font-mono text-[11px] text-fg-secondary">{/not indexed/i.test(err) ? 'run a code_* tool first' : err}</p>
  if (!data) return <p className="p-3 font-mono text-[11px] text-fg-secondary">loading the map…</p>

  const banners = (
    <>
      {(running || line) && (
        <div className="flex h-[18px] shrink-0 items-center gap-2 overflow-hidden border-b border-border-subtle bg-layer-1 px-2 font-mono text-[11px] whitespace-nowrap text-fg-secondary" role="status" data-map-progress>
          {running && (
            <span className="codemap-bar" aria-hidden>
              <i style={{ width: `${progress && progress.total ? Math.round((progress.done / progress.total) * 100) : 8}%` }} />
            </span>
          )}
          <span className="truncate">{line}</span>
          {running && p.has.has('codemap_cancel') && (
            <button type="button" className="ml-auto hover:underline" onClick={() => api.codemap_cancel(root).then(() => setJob(null), (e) => toast.error(String(e)))}>
              cancel
            </button>
          )}
        </div>
      )}
      {!running && asked.current && data.meta.state === 'structure' && !genError && (
        <div className="shrink-0 border-b border-border-subtle bg-layer-1 px-2 py-0.5 font-mono text-[11px] text-fg-secondary">Generating the map in this session’s cost…</div>
      )}
      {genError && (
        <div role="alert" className="flex shrink-0 items-center gap-2 border-b border-border-subtle bg-layer-2 px-2 py-0.5 font-mono text-[11px]">
          <span className="flex-1 truncate" title={genError}>
            {genError}
          </span>
          <button type="button" className="hover:underline" onClick={() => void generate(null)}>
            retry
          </button>
        </div>
      )}
      {stale > 0 && !running && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border-subtle px-2 py-0.5 font-mono text-[11px]">
          <span className="codemap-badge stale">stale · {stale} elements</span>
          <span className="text-fg-secondary">refreshes on the next open</span>
          {p.has.has('codemap_generate') && p.live && (
            <Button size="sm" variant="outline" className="ml-auto h-5 px-1.5 text-[11px]" onClick={() => void generate(null)}>
              Refresh now
            </Button>
          )}
        </div>
      )}
    </>
  )

  const detail = (onClose?: () => void) => (
    <MapDetail element={detailEl} error={detailErr instanceof Error ? detailErr.message : null} root={root} session={session} has={p.has} live={p.live} onDrill={drill} onJump={selectRow} onRegenerate={regenerate} onClose={onClose} />
  )

  // ── phones and narrow panes: the list form ──
  if (p.mobile || p.width < LIST_W) {
    return (
      <div className="flex h-full min-h-0 flex-col" onKeyDown={keys} data-map data-map-form="list">
        {banners}
        {view.data ? (
          <MapList
            element={view.data}
            marks={marks}
            open={sheet}
            detail={sheet ? <MapDetail element={sheet === selectedId ? detailEl : (sel.data ?? null)} error={null} root={root} session={session} has={p.has} live={p.live} onDrill={(id) => (setSheet(null), drill(id))} onJump={(id) => (setSheet(null), selectRow(id))} onRegenerate={regenerate} /> : null}
            onDrill={drill}
            onUp={up}
            onCrumb={drill}
            onOpen={(id) => {
              setSheet(id)
              if (id) go({ selected: id })
            }}
          />
        ) : (
          <p className="p-3 font-mono text-[11px] text-fg-secondary">{view.error ? String(view.error) : 'loading…'}</p>
        )}
      </div>
    )
  }

  // the mock's 880 px pane docks the detail right (260 px); 280 px from 1 080 px; narrower panes stack it under the canvas
  const wide = p.width >= DOCK_W
  return (
    <div className="flex h-full min-h-0 flex-col" onKeyDown={keys} data-map data-map-form="graph">
      {banners}
      <div className="flex min-h-0 flex-1">
        <aside className={cn('flex shrink-0 flex-col border-r border-border-subtle', railOnly ? 'w-7' : 'w-[200px]')} data-map-tree>
          {railOnly ? (
            <button type="button" className="flex-1 font-mono text-[11px] text-fg-secondary hover:bg-layer-2" title="Show the tree" aria-label="Show the tree" onClick={() => setRailOnly(false)}>
              ▸
            </button>
          ) : (
            <>
              <div className="mx-1.5 mt-1.5 mb-1 flex h-[22px] items-center gap-1 rounded border border-border-strong bg-layer-2 px-1.5">
                <input
                  ref={searchRef}
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && hits[0]) {
                      selectRow(hits[0].element)
                      e.stopPropagation()
                    }
                  }}
                  placeholder="Find element or file…"
                  aria-label="Find element or file"
                  className="min-w-0 flex-1 bg-transparent font-mono text-[11px] outline-none placeholder:text-fg-tertiary"
                />
                <button type="button" className="text-fg-tertiary hover:text-foreground" aria-label="Hide the tree" title="Hide the tree" onClick={() => setRailOnly(true)}>
                  ◂
                </button>
              </div>
              {query && hits.length === 0 && <p className="px-2 py-0.5 font-mono text-[10px] text-fg-tertiary">no match</p>}
              <div className="min-h-0 flex-1">
                <MapTree rows={rows} selected={selectedId} chain={chain} status={status} onSelect={selectRow} onToggle={(id, open) => setFolds((f) => new Map(f).set(id, open))} onDrill={drill} onUp={up} />
              </div>
            </>
          )}
        </aside>
        <div className={cn('flex min-h-0 min-w-0 flex-1', wide ? 'flex-row' : 'flex-col')}>
          <div className="min-h-0 min-w-0 flex-1">
            {view.data ? (
              <MapCanvas element={view.data} popped={p.popped} tokens={p.tokens} selected={selectedId !== viewId ? selectedId : null} chain={chain} marks={marks} onSelect={(id) => go({ selected: id ?? viewId })} onDrill={drill} onUp={up} onCrumb={drill} />
            ) : (
              <p className="p-3 font-mono text-[11px] text-fg-secondary">{view.error ? String(view.error instanceof Error ? view.error.message : view.error) : 'loading…'}</p>
            )}
          </div>
          {detailOpen ? (
            <div className={cn('relative shrink-0 overflow-hidden', wide ? cn('border-l border-border-subtle', p.width >= WIDE_W ? 'w-[280px]' : 'w-[260px]') : 'h-[40%] border-t border-border-subtle')} data-map-detail-pane>
              {detail(() => setDetailOpen(false))}
            </div>
          ) : (
            <button
              type="button"
              className={cn('shrink-0 font-mono text-[11px] text-fg-secondary hover:bg-layer-2', wide ? 'w-7 border-l border-border-subtle' : 'h-6 border-t border-border-subtle text-left px-2')}
              aria-label="Show details"
              onClick={() => setDetailOpen(true)}
            >
              {wide ? '◂' : '▴ details'}
            </button>
          )}
        </div>
      </div>
    </div>
  )
}
