import { Link } from '@tanstack/react-router'
import { type CSSProperties, Fragment, type ReactNode, type RefObject, type SVGProps, useEffect, useMemo, useRef, useState } from 'react'
import { age } from '@/components/tasks/helpers'
import { cn } from '@/lib/utils'
import { type Board, C, type Color, FIELD_H, fieldHeight, type Flow, fmtUsd, hhmm, hhmmss, type Hairline, type Need, type PunchCard, type Ring, type SceneData, type SceneModel, type SpendOwnerRow, type SpendPanelModel, type Target, actorLabel, ctxTitle, GAUGE_R, gaugeEnd, gaugePath, projectLine, ringColor, ringDashed, ringDots, SAT_MAX, satPoint, scene, streamColor, streamLink, truncate, W, waffle, WAFFLE_CELLS } from './model'
import type { ActivityEntry } from "@/rpc/bindings/method.roster.output"

/**
 * 128 HUD-SCENE: the visual overview on `/` (≥ md). The flow field is one SVG over a canvas (the grey hairlines) that
 * fills the height it is given: its viewBox is `0 0 1920 hv` with `hv = clamp(round(1920 · h / w), 600, 1400)` from the
 * measured box (R4), and every y in the model is a function of `hv` (`geom`). Below it the band — five 352×248 panels in
 * an HTML grid, four SVG and the STREAM an HTML box so its 50 rows scroll. Every coloured mark decodes to a fact and links
 * to its screen; R2 (Sky): the old sections below are gone, so everything they reached is one click from here.
 */

const LS = { letterSpacing: 1.5 } as const
const pad2 = (n: number) => String(n).padStart(2, '0')

// ---------------------------------------------------------------------------------------------------------------- hooks

/** `Date.now()` once a second; frozen at `fixed` when given (tests, screenshots); paused while the tab is hidden. */
function useClock(fixed: number | undefined): number {
  const [t, setT] = useState(() => fixed ?? Date.now())
  useEffect(() => {
    if (fixed !== undefined) return
    let id: number | undefined
    const stop = () => {
      if (id !== undefined) clearInterval(id)
      id = undefined
    }
    const vis = () => {
      stop()
      if (document.hidden) return
      setT(Date.now())
      id = window.setInterval(() => setT(Date.now()), 1000)
    }
    vis()
    document.addEventListener('visibilitychange', vis)
    return () => {
      stop()
      document.removeEventListener('visibilitychange', vis)
    }
  }, [fixed])
  return t
}

/**
 * R4: the field's viewBox height follows its box — `clamp(round(1920 · h / w), 600, 1400)`, re-measured by a
 * ResizeObserver and applied once per animation frame. Unmeasured (SSR, tests) it is the mockup's 812.
 */
function useFieldHeight(ref: RefObject<HTMLDivElement | null>): number {
  const [hv, setHv] = useState(FIELD_H)
  useEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    let frame = 0
    const measure = () => {
      frame = 0
      setHv(fieldHeight(el.clientWidth, el.clientHeight))
    }
    const ro = new ResizeObserver(() => {
      if (frame === 0) frame = requestAnimationFrame(measure)
    })
    ro.observe(el)
    measure()
    return () => {
      ro.disconnect()
      if (frame !== 0) cancelAnimationFrame(frame)
    }
  }, [ref])
  return hv
}

/** The hairline field on a canvas under the SVG (same `1920 × hv` space): redrawn on data change and at most once a second; the colour is read once per theme change. */
function useField(ref: RefObject<HTMLCanvasElement | null>, lines: readonly Hairline[], hv: number) {
  const color = useRef('')
  const last = useRef(0)
  const [theme, setTheme] = useState(0)
  useEffect(() => {
    const read = () => {
      color.current = getComputedStyle(document.documentElement).getPropertyValue('--fg-primary').trim()
      setTheme((n) => n + 1)
    }
    read()
    const mo = new MutationObserver(read)
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
    return () => mo.disconnect()
  }, [])
  useEffect(() => {
    const draw = () => {
      last.current = Date.now()
      const c = ref.current
      const ctx = c?.getContext('2d')
      if (!c || !ctx) return
      const dpr = window.devicePixelRatio || 1
      if (c.width !== W * dpr || c.height !== hv * dpr) {
        c.width = W * dpr
        c.height = hv * dpr
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, W, hv)
      ctx.strokeStyle = color.current
      ctx.lineWidth = 0.6
      for (const h of lines) {
        ctx.globalAlpha = h.opacity
        ctx.stroke(new Path2D(h.d))
      }
    }
    const wait = 1000 - (Date.now() - last.current)
    if (wait <= 0) {
      draw()
      return
    }
    const t = setTimeout(draw, wait)
    return () => clearTimeout(t)
  }, [ref, lines, theme])
}

// ---------------------------------------------------------------------------------------------------------------- links

/** The typed TanStack `Link` for a `Target` — an SVG `<a>` inside the picture, an HTML one in the band and the compact stack; `label` names an icon-only link. */
export function linkFor(target: Target, children: ReactNode, className?: string, style?: CSSProperties, label?: string): ReactNode {
  const p = { className, style, children, 'aria-label': label }
  switch (target.route) {
    case 'attention':
      return <Link to="/attention/$id" params={{ id: target.id }} {...p} />
    case 'task':
      return <Link to="/tasks/$id" params={{ id: target.id }} {...p} />
    case 'agent':
      return <Link to="/agents/$name" params={{ name: target.name }} {...p} />
    case 'routine':
      return <Link to="/routines/$name" params={{ name: target.name }} {...p} />
    case 'routines':
      return <Link to="/routines" {...p} />
    case 'session':
      return <Link to="/c/$session" params={{ session: target.id }} {...p} />
    case 'agents':
      return <Link to="/agents" {...p} />
    case 'costs':
      return <Link to="/costs" preload="intent" {...p} />
    case 'tasks':
      // The Board-pulse chips' search; a `null` state is the bare board (the route recalls this device's last filters).
      return <Link to="/tasks" search={target.state === null ? {} : { state: target.state, project: '' }} {...p} />
    case 'tasks-project':
      // Revision 6: the BOARD panel's project line. The board has no "no project" filter (`project: ''` is all projects), so `unfiled` opens the bare board.
      return <Link to="/tasks" search={target.project === null ? {} : { project: target.project }} {...p} />
    case 'attention-list':
      return <Link to="/attention" {...p} />
    case 'new':
      return <Link to="/new" {...p} />
  }
}

/** A `<g>` with a tooltip whose content links to `target`; `label` is the link's `aria-label` (icon-only content). */
export function To({ target, title, label, children, ...g }: { target: Target | null; title: string; label?: string; children: ReactNode } & Omit<SVGProps<SVGGElement>, 'target' | 'title'>) {
  return (
    <g {...g}>
      <title>{title}</title>
      {target === null ? children : linkFor(target, children, undefined, undefined, label)}
    </g>
  )
}

// --------------------------------------------------------------------------------------------------------------- panels

/** One 352×248 panel scaled to its cell; the four SVG panels of the band and every panel of the compact stack sit in one. */
export function PanelSvg({ children }: { children: ReactNode }) {
  return (
    <svg viewBox="0 0 352 248" className="block w-full font-mono" role="img">
      {children}
    </svg>
  )
}

/** The panel chrome in local 352×248 coordinates: square, title (+ `action`: a chart-icon link after it, 160 D1), meta, rule, big dim number bottom-right (linked when `bigLink`). */
export function Panel({ title, action, meta, big, bigLink, children }: { title: string; action?: { target: Target; title: string }; meta: string; big: string; bigLink?: Target; children: ReactNode }) {
  // The action icon sits after the title: mono at fontSize 9 with letterSpacing 1.5 is ≈ 6.9 units a glyph.
  const ax = 24 + Math.round(title.length * 6.9) + 5
  const number = (
    <text x={340} y={236} fontSize={18} fill={C.borderStrong} textAnchor="end">
      {big}
    </text>
  )
  return (
    <g>
      <rect width={352} height={248} fill={C.elevated} stroke={C.border} />
      <rect x={12} y={13} width={5} height={5} fill={C.tertiary} />
      <text x={24} y={19} fontSize={9} fill={C.primary} style={LS}>
        {title}
      </text>
      {action && (
        <To target={action.target} title={action.title} label={action.title}>
          <rect x={ax} y={9} width={14} height={14} rx={3} fill={C.elevated} stroke={C.borderStrong} />
          {/* lucide `chart-line`, 24-box scaled to 9 units inside the square */}
          <path transform={`translate(${ax + 1} 10) scale(0.5)`} d="M3 3v18h18M19 9l-5 5-4-4-3 3" fill="none" stroke={C.secondary} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" />
        </To>
      )}
      <text x={340} y={19} fontSize={8} fill={C.tertiary} textAnchor="end" style={{ letterSpacing: 1 }}>
        {meta}
      </text>
      <line x1={12} y1={28} x2={340} y2={28} stroke={C.border} />
      {children}
      {bigLink ? (
        <To target={bigLink} title={`${title} · ${big}`}>
          {number}
        </To>
      ) : (
        number
      )}
    </g>
  )
}

export function AgentsPanel({ rings, activityToday }: { rings: readonly Ring[]; activityToday: readonly ActivityEntry[] }) {
  const live = rings.filter((r) => r.style === 'live').length
  return (
    <Panel title="AGENTS" meta={`${live} LIVE`} big={`${live} live`} bigLink={{ route: 'agents' }}>
      {rings.slice(0, 6).map((r, i) => {
        const y = 48 + 32 * i
        return (
          <To key={r.id} target={r.link} title={`${r.name} · ${r.sub}`}>
            <text x={14} y={y} fontSize={10} fill={r.style === 'lapsed' ? C.tertiary : C.primary}>
              {r.name}
            </text>
            <text x={340} y={y} fontSize={9} fill={C.secondary} textAnchor="end">
              {r.sub}
            </text>
            {ringDots(r, activityToday).map((d, j) => (
              <circle key={j} cx={16 + 11 * j} cy={y + 10} r={2.6} fill={d.style === 'done' || d.style === 'raised' ? d.color : 'none'} stroke={d.style === 'done' || d.style === 'raised' ? undefined : d.color} strokeWidth={1} strokeDasharray={d.style === 'lapsed' ? '1.5 1.5' : undefined} />
            ))}
          </To>
        )
      })}
    </Panel>
  )
}

const DAYS = ['m', 't', 'w', 't', 'f', 's', 's']
export function ArrivalsPanel({ punch, now }: { punch: PunchCard; now: number }) {
  const off = -new Date(now).getTimezoneOffset() / 60
  const [nd, nh] = punch.now
  return (
    <Panel title="ARRIVALS" meta={`UTC${off >= 0 ? '+' : '−'}${Math.abs(off)}`} big={hhmm(now)}>
      {punch.cells.map((row, d) => (
        <g key={d}>
          <text x={14} y={55 + 24 * d} fontSize={9} fill={C.tertiary}>
            {DAYS[d]}
          </text>
          {row.map((n, h) => {
            const v = punch.max > 0 ? n / punch.max : 0
            return <circle key={h} cx={34 + 13.2 * h} cy={52 + 24 * d} r={0.6 + 2.6 * v} fill={C.secondary} fillOpacity={0.25 + 0.75 * v} />
          })}
        </g>
      ))}
      <circle cx={34 + 13.2 * nh} cy={52 + 24 * nd} r={6} fill="none" stroke={C.primary} strokeWidth={1} />
      {[0, 6, 12, 18].map((h) => (
        <text key={h} x={34 + 13.2 * h} y={226} fontSize={8} fill={C.tertiary} textAnchor="middle">
          {pad2(h)}
        </text>
      ))}
    </Panel>
  )
}

/**
 * Revision 6: the whole board, every project. The waffle is one cell per task in board order (scaled by `waffle` past 60), the
 * legend one row per state with its all-time count (each opens that column), the big number the board's size, the bottom line
 * the projects by task count (each opens that project's board), and under the waffle today's movement.
 */
export function BoardPanel({ board }: { board: Board }) {
  const fills: { color: Color; opacity?: number }[] = []
  waffle(board.byState.map((s) => s.n)).forEach((n, i) => {
    const s = board.byState[i]!
    for (let k = 0; k < n; k++) fills.push(s)
  })
  const line = projectLine(board.projects)
  return (
    <Panel title="BOARD" meta={`${board.projects.length} PROJECTS`} big={String(board.total)} bigLink={{ route: 'tasks', state: null }}>
      {Array.from({ length: WAFFLE_CELLS }, (_, i) => {
        const f = fills[i]
        return <circle key={i} cx={20 + 14 * (i % 10)} cy={50 + 14 * Math.floor(i / 10)} r={3.2} fill={f?.color ?? 'none'} fillOpacity={f?.opacity} stroke={f ? undefined : C.borderStrong} strokeWidth={1} />
      })}
      {board.byState.map((s, k) => (
        <To key={s.state} target={{ route: 'tasks', state: s.state }} title={`${s.n} ${s.state} · open the board at ${s.state}`}>
          <circle cx={186} cy={49 + 14 * k} r={2.5} fill={s.color} fillOpacity={s.opacity} />
          <text x={196} y={52 + 14 * k} fontSize={10} fill={C.secondary}>
            {s.state}
          </text>
          <text x={338} y={52 + 14 * k} fontSize={10} fill={C.primary} textAnchor="end">
            {s.n}
          </text>
        </To>
      ))}
      <To target={{ route: 'tasks', state: null }} title={`${board.movedToday} tasks changed since midnight · open the board`}>
        <text x={20} y={136} fontSize={9} fill={C.tertiary}>
          {board.movedToday} moved today
        </text>
      </To>
      <text x={20} y={148} fontSize={9} fill={C.tertiary} data-projects>
        {line.shown.map((p, i) => (
          <Fragment key={p.name ?? ''}>
            {i > 0 && <tspan> · </tspan>}
            {linkFor(
              { route: 'tasks-project', project: p.name },
              <>
                <title>{p.name === null ? `${p.n} tasks in no project · open the board` : `${p.n} tasks · open the board at ${p.name}`}</title>
                <tspan>{p.label}</tspan>
              </>,
            )}
          </Fragment>
        ))}
        {line.more > 0 && <tspan> +{line.more}</tspan>}
      </text>
    </Panel>
  )
}

/** Where an owner row goes: an agent to its page, a routine to its page, `you`/`other` nowhere (there is no sessions list route). */
export const ownerTarget = (o: SpendOwnerRow): Target | null => (o.kind === 'agent' ? { route: 'agent', name: o.name } : o.kind === 'routine' ? { route: 'routine', name: o.name } : null)

const BAR_SLOT = 324 / 7
const BAR_W = 28
const BAR_TOP = 40
const BAR_H = 52
const OWNER_Y0 = 124
const OWNER_DY = 16

/**
 * Rev 3: SPEND reads the model-call ledger (`spend_report`). Big number = month to date, meta = today; seven thin bars for
 * the last local days scaled to the week's max (today faience, the rest secondary ink; a $0 day is a hairline, never blank);
 * then the owner rows (ring dot = agent, square = routine, plain = you) and, only for agents with a monthly cap, the budget
 * rows with their share bar. No gold and no carnelian here: nothing in this panel is a need or an error.
 */
export function SpendPanel({ p }: { p: SpendPanelModel }) {
  const max = Math.max(0, ...p.days.map((d) => d.usd))
  const capsY0 = OWNER_Y0 + OWNER_DY * p.owners.length + 8
  const caps = p.caps.map((r, i) => ({ r, y: capsY0 + 18 * i })).filter(({ y }) => y + 9 <= 212)
  return (
    <Panel title="SPEND" action={{ target: { route: 'costs' }, title: 'Open costs' }} meta={`TODAY ${fmtUsd(p.today)}`} big={fmtUsd(p.month)} bigLink={{ route: 'costs' }}>
      {p.days.map((d, i) => {
        const x = 14 + BAR_SLOT * i + (BAR_SLOT - BAR_W) / 2
        const h = max > 0 ? Math.max(1, (BAR_H * d.usd) / max) : 1
        const ink = d.today ? C.info : C.secondary
        return (
          <To key={i} target={null} title={`${d.today ? 'today' : d.label} · ${fmtUsd(d.usd)}`}>
            <rect x={x} y={BAR_TOP + BAR_H - h} width={BAR_W} height={h} fill={ink} />
            <text x={x + BAR_W / 2} y={BAR_TOP + BAR_H + 12} fontSize={8} fill={d.today ? C.info : C.tertiary} textAnchor="middle">
              {d.label}
            </text>
          </To>
        )
      })}
      {p.owners.map((o, i) => {
        const y = OWNER_Y0 + OWNER_DY * i
        return (
          <To key={`${o.kind}:${o.name}`} target={ownerTarget(o)} title={`${o.name} · ${fmtUsd(o.usd)} this week`}>
            {o.kind === 'agent' && <circle cx={18} cy={y - 3.5} r={3} fill="none" stroke={C.secondary} strokeWidth={1} />}
            {o.kind === 'routine' && <rect x={15.5} y={y - 6} width={5} height={5} fill={C.secondary} />}
            <text x={28} y={y} fontSize={10} fill={C.primary}>
              {truncate(o.name, 28)}
            </text>
            <text x={338} y={y} fontSize={9} fill={C.secondary} textAnchor="end">
              {fmtUsd(o.usd)}
            </text>
          </To>
        )
      })}
      {caps.map(({ r, y }) => (
        <To key={r.name} target={{ route: 'agent', name: r.name }} title={`${r.name} · $${r.spent.toFixed(2)} of $${r.cap} this month`}>
          <text x={14} y={y} fontSize={9} fill={C.tertiary}>
            {r.name} cap
          </text>
          <text x={340} y={y} fontSize={9} fill={C.tertiary} textAnchor="end">
            {`$${r.spent.toFixed(1)} / $${r.cap}`}
          </text>
          <line x1={14} y1={y + 5} x2={338} y2={y + 5} stroke={C.borderStrong} strokeWidth={2} />
          {r.share !== null && r.share > 0 && <line x1={14} y1={y + 5} x2={14 + 324 * r.share} y2={y + 5} stroke={r.share > 0.85 ? C.primary : C.success} strokeWidth={2} />}
        </To>
      ))}
    </Panel>
  )
}

/** Container-query units: the panel's 352-wide local coordinate expressed against the cell's width, so HTML text scales like the SVG panels beside it. */
const u = (px: number) => `${((px * 100) / 352).toFixed(3)}cqw`

/**
 * The STREAM panel as an HTML box (R2): all 50 feed rows, newest first, live-prepended by `useActivity`, scrolling inside the
 * panel's frame; each row links to its subject. `className` sizes the frame — the band's cell locks the 352×248 aspect, the
 * compact stack caps it at 40 % of the viewport.
 */
export function StreamPanel({ activity, className }: { activity: readonly ActivityEntry[]; className?: string }) {
  return (
    <section aria-label="Stream" className={cn('@container relative flex flex-col overflow-hidden border font-mono', className)} style={{ background: C.elevated, borderColor: C.border }}>
      <header className="flex shrink-0 items-center border-b" style={{ height: u(28), paddingLeft: u(12), paddingRight: u(12), borderColor: C.border }}>
        <span className="shrink-0" style={{ width: u(5), height: u(5), background: C.tertiary }} />
        <span style={{ marginLeft: u(7), fontSize: u(9), letterSpacing: u(1.5), color: C.primary }}>STREAM</span>
        <span className="ml-auto" style={{ fontSize: u(8), letterSpacing: u(1), color: C.tertiary }}>
          LIVE
        </span>
      </header>
      <ol className="min-h-0 flex-1 overflow-y-auto" style={{ paddingTop: u(10), paddingBottom: u(24) }}>
        {activity.map((e) => {
          const color = streamColor(e.kind)
          const link = streamLink(e)
          const row = (
            <>
              <span className="shrink-0 tabular-nums" style={{ width: u(62), color: C.tertiary }}>
                {hhmmss(e.ts_ms)}
              </span>
              <span className="shrink-0 truncate" style={{ width: u(84), color }}>
                {actorLabel(e.actor)}
              </span>
              <span className="min-w-0 flex-1 truncate" style={{ color }}>
                {e.summary}
              </span>
            </>
          )
          const cls = 'flex items-center'
          const style = { height: u(20), paddingLeft: u(14), paddingRight: u(12), fontSize: u(9) }
          return (
            <li key={e.id} title={`${hhmmss(e.ts_ms)} · ${actorLabel(e.actor)} · ${e.summary}${e.cost === null ? '' : ` · $${e.cost.toFixed(2)}`}`}>
              {link === null ? (
                <div className={cls} style={style}>
                  {row}
                </div>
              ) : (
                linkFor(link, row, cn(cls, 'hover:bg-accent'), style)
              )}
            </li>
          )
        })}
      </ol>
      <span aria-hidden className="pointer-events-none absolute" style={{ right: u(12), bottom: u(6), fontSize: u(18), lineHeight: 1, color: C.borderStrong }}>
        {activity.length}/50
      </span>
    </section>
  )
}

// ---------------------------------------------------------------------------------------------------------------- scene

/** The ring glyph at (x, y): filled + pulsing when live, hollow when idle, gold when waiting, dashed when lapsed/error, hollow carnelian when a routine failed; `on` (R5) widens the stroke to 2. */
export function RingGlyph({ r, x, y, on = false }: { r: Ring; x: number; y: number; on?: boolean }) {
  const color = ringColor(r.style)
  const live = r.style === 'live' || r.style === 'waiting'
  return (
    <>
      <circle cx={x} cy={y} r={13} fill={C.page} stroke={color} strokeWidth={on ? 2 : live ? 1.6 : 1.2} strokeDasharray={ringDashed(r.style) ? '3 3' : undefined} />
      {live && <circle cx={x} cy={y} r={5} fill={color} className={r.style === 'live' ? 'hud-pulse' : undefined} />}
    </>
  )
}

/** The footer's six segments, every one a link (R2); the last two share the right-aligned slot. */
export function footerSegments(m: SceneModel): { text: string; color: Color; target: Target }[] {
  const f = m.footer
  return [
    { text: `${f.sessionsLive} SESSIONS LIVE`, color: C.tertiary, target: { route: 'new' } },
    { text: `${f.agentsWorking} AGENTS WORKING`, color: C.tertiary, target: { route: 'agents' } },
    { text: `${f.needYou === null ? '—' : pad2(f.needYou)} NEED YOU`, color: f.needYou !== null && f.needYou > 0 ? C.accent : C.tertiary, target: { route: 'attention-list' } },
    { text: `${fmtUsd(f.month)} THIS MONTH`, color: C.tertiary, target: { route: 'costs' } },
    { text: `${f.outcomes} OUTCOMES TODAY`, color: C.tertiary, target: { route: 'tasks', state: 'done' } },
    { text: `${f.inFlight} IN FLIGHT`, color: C.tertiary, target: { route: 'tasks', state: 'doing' } },
  ]
}

/** The five band cells: the four SVG panels and the STREAM box; `stream` sizes the STREAM frame for its host. */
export function Panels({ data, m, now, stream }: { data: SceneData; m: SceneModel; now: number; stream: string }) {
  return (
    <>
      <PanelSvg>
        <AgentsPanel rings={m.rings} activityToday={data.activityToday ?? []} />
      </PanelSvg>
      <PanelSvg>
        <ArrivalsPanel punch={m.punch} now={now} />
      </PanelSvg>
      <PanelSvg>
        <BoardPanel board={m.board} />
      </PanelSvg>
      <PanelSvg>
        <SpendPanel p={m.spend} />
      </PanelSvg>
      <StreamPanel activity={data.activity ?? []} className={stream} />
    </>
  )
}

const needOf = (m: SceneModel, fl: Flow): Need | undefined => (fl.kind === 'need' ? m.needs.find((n) => `need:${n.item.id}` === fl.id) : undefined)

/** One flow's visible marks: the stroke (inert — the hit path takes the pointer), end dot, label, and for a need its dot; `on` = the R5 hovered look. */
function FlowBody({ fl, need, on }: { fl: Flow; need: Need | undefined; on: boolean }) {
  if (fl.kind === 'due') {
    // Revision 1: a due-later routine's faint connector — ring → its hollow square, nothing else.
    return <path d={fl.d} stroke={fl.color} strokeOpacity={on ? 1 : 0.5} strokeWidth={on ? 1.8 : 1.1} strokeDasharray="3 3" fill="none" pointerEvents="none" />
  }
  return (
    <>
      <path d={fl.d} stroke={fl.color} strokeOpacity={on ? 1 : 0.95} strokeWidth={on ? 1.8 : 1.1} fill="none" pointerEvents="none" />
      <circle cx={fl.end[0]} cy={fl.end[1]} r={on ? 2.6 : 1.8} fill={fl.color} />
      <text x={fl.end[0] + 9} y={fl.end[1] + 3.5} fontSize={10} fill={on ? C.primary : fl.color} fontWeight={on ? 500 : undefined}>
        {fl.label}
      </text>
      {need && (
        <>
          <circle cx={need.x} cy={need.y} r={7} fill={C.page} stroke={C.accent} strokeWidth={on ? 2 : 1.2} />
          <circle cx={need.x} cy={need.y} r={1.6} fill={C.accent} />
          <text x={need.x - 13} y={need.y + 4} fontSize={10} fill={on ? C.primary : C.accent} fontWeight={on ? 500 : undefined} textAnchor="end">
            {need.label}
          </text>
        </>
      )}
    </>
  )
}

/**
 * R5: the hovered (or focused) flow is state, not CSS — it is drawn last (on top), widened, with both its ends emphasised
 * (the need dot / the via ring on the left, the label on the right) while every other flow dims to 0.35. `hot` seeds the
 * state for tests and screenshots, as `now` does for the clock.
 */
export function Scene({ data, errors = [], now: fixed, hot: seed = null }: { data: SceneData; errors?: string[]; now?: number; hot?: string | null }) {
  const clock = useClock(fixed)
  const minute = clock - (clock % 60_000)
  const field = useRef<HTMLDivElement>(null)
  const hv = useFieldHeight(field)
  const m = useMemo(() => scene(data, minute, hv), [data, minute, hv])
  const canvas = useRef<HTMLCanvasElement>(null)
  useField(canvas, m.hairlines, hv)
  const [hot, setHot] = useState<string | null>(seed)
  const hotFlow = hot === null ? undefined : m.flows.find((fl) => fl.id === hot)
  const hotRing = hotFlow?.via?.id ?? null
  const f = m.footer
  const need = f.needYou ?? 0
  const summary = `${f.needYou ?? '—'} need you · ${f.agentsWorking} agents working · ${f.sessionsLive} sessions live · ${f.outcomes} outcomes today · ${f.inFlight} in flight · $${f.month.toFixed(2)} this month`
  const segs = footerSegments(m)
  const segX = [28, 190, 356, 470]
  return (
    <div className="hud-scene flex flex-1 flex-col">
      <div ref={field} className="relative min-h-[420px] flex-1">
        <canvas ref={canvas} width={W} height={hv} aria-hidden className="absolute inset-0 h-full w-full object-contain" />
        <svg viewBox={`0 0 ${W} ${hv}`} className="absolute inset-0 block h-full w-full font-mono" role="img" aria-labelledby="hud-scene-title hud-scene-desc">
          <title id="hud-scene-title">Overview</title>
          <desc id="hud-scene-desc">{summary}</desc>

          {/* top bar */}
          <text x={24} y={18} fontSize={10} fill={C.accent} style={{ letterSpacing: 2 }}>
            T H E U T H
          </text>
          <line x1={1660} y1={14} x2={1840} y2={14} stroke={C.borderStrong} strokeDasharray="2 4" />
          <text x={1896} y={18} fontSize={10} fill={C.secondary} textAnchor="end">
            <tspan fill={need > 0 ? C.accent : C.secondary}>●</tspan> {hhmmss(clock)}
          </text>
          {[
            [14, 54, 14, 40, 28, 40],
            [1906, 54, 1906, 40, 1892, 40],
            [14, hv - 54, 14, hv - 40, 28, hv - 40],
            [1906, hv - 54, 1906, hv - 40, 1892, hv - 40],
          ].map((p, i) => (
            <path key={i} d={`M ${p[0]} ${p[1]} L ${p[2]} ${p[3]} L ${p[4]} ${p[5]}`} stroke={C.borderStrong} fill="none" />
          ))}

          {/* (b) the day arc */}
          <path d={m.arc.band} fill={C.elevated} stroke={C.borderStrong} />
          {m.arc.ticks.map((t, i) => (
            <line key={i} x1={t.x1} y1={t.y1} x2={t.x2} y2={t.y2} stroke={C.tertiary} />
          ))}
          {m.arc.labels.map((l) => (
            <text key={l.text} x={l.x} y={l.y} fontSize={8} fill={C.tertiary} textAnchor="middle" transform={`rotate(${l.rot.toFixed(1)} ${l.x.toFixed(1)} ${l.y.toFixed(1)})`}>
              {l.text}
            </text>
          ))}
          {m.arc.earlier && (
            <text x={m.arc.earlier.x} y={m.arc.earlier.y} fontSize={8} fill={C.tertiary} textAnchor="middle" transform={`rotate(${m.arc.earlier.rot.toFixed(1)} ${m.arc.earlier.x.toFixed(1)} ${m.arc.earlier.y.toFixed(1)})`}>
              earlier
            </text>
          )}
          {m.arc.marks.map((k, i) => (
            <To key={`${k.name}:${i}`} target={k.link} title={`${k.name} · ${k.filled ? 'fired today' : 'due later today'}`}>
              <rect x={k.x - 2.5} y={k.y - 2.5} width={5} height={5} fill={k.filled ? C.primary : 'none'} stroke={C.primary} strokeWidth={0.8} transform={`rotate(${k.rot.toFixed(1)} ${k.x.toFixed(1)} ${k.y.toFixed(1)})`} />
              <text x={k.lx} y={k.ly} fontSize={7} fill={k.filled ? C.secondary : C.tertiary} textAnchor="middle" transform={`rotate(${k.lrot.toFixed(1)} ${k.lx.toFixed(1)} ${k.ly.toFixed(1)})`}>
                {k.name}
              </text>
            </To>
          ))}
          <line x1={m.arc.now.x1} y1={m.arc.now.y1} x2={m.arc.now.x2} y2={m.arc.now.y2} stroke={C.primary} strokeWidth={1.5} />
          <text x={m.arc.now.lx} y={m.arc.now.ly} fontSize={9} fill={C.primary} transform={`rotate(${m.arc.now.lrot.toFixed(1)} ${m.arc.now.lx.toFixed(1)} ${m.arc.now.ly.toFixed(1)})`}>
            now
          </text>

          {/* (c) coloured flows, (d) their needs-you dots. R5: an invisible 8 px hit stroke first in each group (1 px lines are hoverable), the
              visible stroke on top and inert; hover/focus set `hot` — the rest dim to 0.35 and that flow is drawn AGAIN after them (`FlowBody`
              below), widened with both ends emphasised. A copy, not a reorder: moving the node would restart its `hud-flow-in` fade. */}
          {m.flows.map((fl) => (
            <To
              key={fl.id}
              target={fl.link}
              title={fl.title}
              data-flow={fl.id}
              className="hud-flow-in"
              style={hotFlow && fl !== hotFlow ? { opacity: 0.35 } : undefined}
              onPointerEnter={() => setHot(fl.id)}
              onPointerLeave={() => setHot(null)}
              onFocus={() => setHot(fl.id)}
              onBlur={() => setHot(null)}
            >
              <path d={fl.d} stroke="transparent" strokeWidth={8} fill="none" pointerEvents="stroke" />
              <FlowBody fl={fl} need={needOf(m, fl)} on={false} />
            </To>
          ))}
          {hotFlow && (
            <g data-hot={hotFlow.id} pointerEvents="none">
              <FlowBody fl={hotFlow} need={needOf(m, hotFlow)} on />
            </g>
          )}
          {m.flowsMore > 0 && (
            <text x={1500} y={52} fontSize={9} fill={C.tertiary}>
              +{m.flowsMore} more
            </text>
          )}

          {/* (e) rings, (f) their labels, (g) Revision 2: the desk's context gauge on the outer track — length-coded only, faience when live, else the secondary ink;
              (h) Revision 4: a root session's live subagents as satellites on r 23 from 3 o'clock clockwise, 8 then `+N`; R5: the hovered flow's via ring (`data-ring`) is emphasised — stroke 2, labels 500. */}
          {m.rings.map((r) => {
            const sats = r.children ?? []
            const lx = r.x - (sats.length > 0 ? 30 : 22)
            const on = r.id === hotRing
            return (
              <To key={r.id} target={r.link} title={`${r.name} · ${r.sub || r.style}${r.ctx ? ` · ${ctxTitle(r.ctx)}` : ''}`} data-ring={r.id}>
                {r.ctx && (
                  <g data-gauge={r.id}>
                    <circle cx={r.x} cy={r.y} r={GAUGE_R} fill="none" stroke={C.border} strokeWidth={1} />
                    {r.ctx.fill > 0 && <path d={gaugePath(r.x, r.y, GAUGE_R, r.ctx.fill)} fill="none" stroke={r.style === 'live' ? C.info : C.secondary} strokeWidth={2} strokeLinecap="round" />}
                    <circle cx={gaugeEnd(r.x, r.y, GAUGE_R, r.ctx.fill)[0]} cy={gaugeEnd(r.x, r.y, GAUGE_R, r.ctx.fill)[1]} r={1.6} fill={r.style === 'live' ? C.info : C.secondary} />
                  </g>
                )}
                {sats.length > 0 && (
                  <g data-satellites={r.id}>
                    {sats.slice(0, SAT_MAX).map((c, i) => {
                      const [sx, sy] = satPoint(r.x, r.y, i)
                      return (
                        <circle key={c.id} cx={sx} cy={sy} r={2.2} fill={c.running ? C.info : 'none'} stroke={c.running ? undefined : C.secondary} strokeWidth={1}>
                          <title>{`${c.title} · ${c.running ? 'running' : 'idle'}`}</title>
                        </circle>
                      )
                    })}
                    {sats.length > SAT_MAX && (
                      <text x={satPoint(r.x, r.y, SAT_MAX)[0]} y={satPoint(r.x, r.y, SAT_MAX)[1] + 2.5} fontSize={7} fill={C.secondary} textAnchor="middle">
                        +{sats.length - SAT_MAX}
                      </text>
                    )}
                  </g>
                )}
                <RingGlyph r={r} x={r.x} y={r.y} on={on} />
                <text x={lx} y={r.y - 1} fontSize={11} fill={r.style === 'lapsed' && !on ? C.tertiary : C.primary} fontWeight={on ? 500 : undefined} textAnchor="end">
                  {r.name}
                </text>
                <text x={lx} y={r.y + 12} fontSize={9} fill={ringColor(r.style)} fontWeight={on ? 500 : undefined} textAnchor="end">
                  {r.sub}
                </text>
              </To>
            )
          })}

          {/* NEEDS YOU table — the header opens the inbox, every row its item */}
          <To target={{ route: 'attention-list' }} title="Open the inbox">
            <text x={28} y={60} fontSize={9} fill={C.tertiary} style={LS}>
              NEEDS YOU
            </text>
            <text x={430} y={60} fontSize={9} fill={need > 0 ? C.accent : C.tertiary} textAnchor="end">
              {f.needYou === null ? '—' : pad2(f.needYou)}
            </text>
          </To>
          <line x1={28} y1={66} x2={430} y2={66} stroke={C.border} />
          {(['ID', 'KIND', 'AGE', 'TITLE'] as const).map((h, i) => (
            <text key={h} x={[28, 90, 160, 200][i]} y={82} fontSize={9} fill={C.tertiary}>
              {h}
            </text>
          ))}
          {f.needYou === 0 && (
            <text x={28} y={100} fontSize={11} fill={C.tertiary}>
              Nothing needs you
            </text>
          )}
          {m.needs.slice(0, 6).map((n, i) => (
            <To key={n.item.id} target={n.link} title={`${n.item.kind} · ${n.item.title}${n.item.detail ? ` — ${n.item.detail}` : ''}`}>
              <text x={28} y={100 + 17 * i} fontSize={11} fill={C.accent}>
                {n.label}
              </text>
              <text x={90} y={100 + 17 * i} fontSize={11} fill={C.accent}>
                {n.item.kind}
              </text>
              <text x={160} y={100 + 17 * i} fontSize={11} fill={C.accent}>
                {age(n.item.at_ms, minute)}
              </text>
              <text x={200} y={100 + 17 * i} fontSize={11} fill={C.accent}>
                {truncate(n.item.title, 38)}
              </text>
            </To>
          ))}
          {need > 6 && (
            <To target={{ route: 'attention-list' }} title={`${need - 6} more in the inbox`}>
              <text x={28} y={100 + 17 * 6} fontSize={11} fill={C.accent}>
                +{need - 6}
              </text>
            </To>
          )}

          {/* footer: four segments from the left, the last two sharing the right-aligned slot */}
          <line x1={24} y1={hv - 34} x2={1896} y2={hv - 34} stroke={C.border} />
          {segs.slice(0, 4).map((s, i) => (
            <To key={s.text} target={s.target} title={s.text}>
              <text x={segX[i]} y={hv - 17} fontSize={9} fill={s.color} style={LS}>
                {s.text}
              </text>
            </To>
          ))}
          <text x={1896} y={hv - 17} fontSize={9} fill={C.tertiary} textAnchor="end" style={LS}>
            {linkFor(segs[4]!.target, <tspan>{segs[4]!.text}</tspan>)}
            <tspan> · </tspan>
            {linkFor(segs[5]!.target, <tspan>{segs[5]!.text}</tspan>)}
          </text>
        </svg>
      </div>

      {/* the bottom band: five 352×248 cells at the mockup's 24 px margins and 28 px gaps, as fractions of 1920 */}
      <div className="grid grid-cols-5 gap-[1.458%] px-[1.25%] pb-[1.04%]">
        <Panels data={data} m={m} now={clock} stream="aspect-[352/248]" />
      </div>
      {errors.length > 0 && (
        <p className="mt-1 px-[1.25%] font-mono text-xs" style={{ color: C.danger }}>
          {errors.join(' · ')} failed to load
        </p>
      )}
    </div>
  )
}
