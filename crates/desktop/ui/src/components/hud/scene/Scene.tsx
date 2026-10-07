import { Link } from '@tanstack/react-router'
import { type CSSProperties, type ReactNode, type RefObject, type SVGProps, useEffect, useMemo, useRef, useState } from 'react'
import { age } from '@/components/tasks/helpers'
import { cn } from '@/lib/utils'
import { spend } from '../model'
import { C, type Color, FIELD_H, fieldHeight, hhmm, hhmmss, MIX_ORDER, type Hairline, type OutcomeMix, type PunchCard, type Ring, type SceneData, type SceneModel, type Target, actorLabel, ctxTitle, GAUGE_R, gaugeEnd, gaugePath, ringColor, ringDashed, ringDots, SAT_MAX, satPoint, scene, streamColor, streamLink, truncate, W } from './model'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { Task } from '@/rpc/bindings/Task'

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

/** The typed TanStack `Link` for a `Target` — an SVG `<a>` inside the picture, an HTML one in the band and the compact stack. */
export function linkFor(target: Target, children: ReactNode, className?: string, style?: CSSProperties): ReactNode {
  const p = { className, style, children }
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
    case 'tasks':
      // The Board-pulse chips' search; a `null` state is the bare board (the route recalls this device's last filters).
      return <Link to="/tasks" search={target.state === null ? {} : { state: target.state, project: '' }} {...p} />
    case 'attention-list':
      return <Link to="/attention" {...p} />
    case 'new':
      return <Link to="/new" {...p} />
  }
}

/** A `<g>` with a tooltip whose content links to `target`. */
export function To({ target, title, children, ...g }: { target: Target | null; title: string; children: ReactNode } & Omit<SVGProps<SVGGElement>, 'target' | 'title'>) {
  return (
    <g {...g}>
      <title>{title}</title>
      {target === null ? children : linkFor(target, children)}
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

/** The panel chrome in local 352×248 coordinates: square, title, meta, rule, big dim number bottom-right (linked when `bigLink`). */
export function Panel({ title, meta, big, bigLink, children }: { title: string; meta: string; big: string; bigLink?: Target; children: ReactNode }) {
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

/** The waffle of today's moves; each legend row opens that board column, the dim line under it the backlog/todo columns, the big number the board. */
export function OutcomesPanel({ mix, tasks }: { mix: OutcomeMix; tasks: readonly Task[] }) {
  const total = MIX_ORDER.reduce((s, m) => s + mix[m.key], 0)
  const scale = total > 60 ? 60 / total : 1
  const fills: Color[] = []
  for (const m of MIX_ORDER) for (let i = 0; i < Math.round(mix[m.key] * scale) && fills.length < 60; i++) fills.push(m.color)
  const backlog = tasks.filter((t) => t.state === 'backlog').length
  const todo = tasks.filter((t) => t.state === 'todo').length
  return (
    <Panel title="OUTCOMES" meta={`OF ${total}`} big={String(total)} bigLink={{ route: 'tasks', state: null }}>
      {Array.from({ length: 60 }, (_, i) => {
        const fill = fills[i]
        return <circle key={i} cx={20 + 14 * (i % 10)} cy={50 + 14 * Math.floor(i / 10)} r={3.2} fill={fill ?? 'none'} stroke={fill ? undefined : C.borderStrong} strokeWidth={1} />
      })}
      {MIX_ORDER.map((m, k) => (
        <To key={m.key} target={{ route: 'tasks', state: m.state }} title={`${mix[m.key]} ${m.key} · open the board at ${m.state}`}>
          <circle cx={186} cy={49 + 18 * k} r={2.5} fill={m.color} />
          <text x={196} y={52 + 18 * k} fontSize={10} fill={C.secondary}>
            {m.key}
          </text>
          <text x={338} y={52 + 18 * k} fontSize={10} fill={C.primary} textAnchor="end">
            {mix[m.key]}
          </text>
        </To>
      ))}
      <To target={{ route: 'tasks', state: 'backlog' }} title={`${backlog} in the backlog`}>
        <text x={196} y={148} fontSize={9} fill={C.tertiary}>
          {backlog} backlog
        </text>
      </To>
      <To target={{ route: 'tasks', state: 'todo' }} title={`${todo} to do`}>
        <text x={338} y={148} fontSize={9} fill={C.tertiary} textAnchor="end">
          {todo} todo
        </text>
      </To>
    </Panel>
  )
}

export function SpendPanel({ agents }: { agents: readonly AgentSummary[] }) {
  const rows = spend(agents)
  const total = agents.reduce((s, a) => s + a.spent_month, 0)
  return (
    <Panel title="SPEND" meta="MONTH" big={`$${total.toFixed(2)}`} bigLink={{ route: 'agents' }}>
      {rows.slice(0, 5).map((r, i) => {
        const y = 50 + 32 * i
        return (
          <To key={r.name} target={{ route: 'agent', name: r.name }} title={`${r.name} · $${r.spent.toFixed(2)}${r.cap === null ? '' : ` of $${r.cap}`} this month`}>
            <text x={14} y={y} fontSize={10} fill={C.primary}>
              {r.name}
            </text>
            <text x={340} y={y} fontSize={9} fill={C.secondary} textAnchor="end">
              {r.cap === null ? `$${r.spent.toFixed(1)} · no cap` : `$${r.spent.toFixed(1)} / $${r.cap}`}
            </text>
            <line x1={14} y1={y + 9} x2={338} y2={y + 9} stroke={C.borderStrong} strokeWidth={2} />
            {r.share !== null && r.share > 0 && <line x1={14} y1={y + 9} x2={14 + 324 * r.share} y2={y + 9} stroke={r.share > 0.85 ? C.danger : C.success} strokeWidth={2} />}
          </To>
        )
      })}
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

/** The ring glyph at (x, y): filled + pulsing when live, hollow when idle, gold when waiting, dashed when lapsed/error, hollow carnelian when a routine failed. */
export function RingGlyph({ r, x, y }: { r: Ring; x: number; y: number }) {
  const color = ringColor(r.style)
  const live = r.style === 'live' || r.style === 'waiting'
  return (
    <>
      <circle cx={x} cy={y} r={13} fill={C.page} stroke={color} strokeWidth={live ? 1.6 : 1.2} strokeDasharray={ringDashed(r.style) ? '3 3' : undefined} />
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
    { text: `$${f.month.toFixed(2)} THIS MONTH`, color: C.tertiary, target: { route: 'agents' } },
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
        <OutcomesPanel mix={m.mix} tasks={data.tasks ?? []} />
      </PanelSvg>
      <PanelSvg>
        <SpendPanel agents={data.agents ?? []} />
      </PanelSvg>
      <StreamPanel activity={data.activity ?? []} className={stream} />
    </>
  )
}

export function Scene({ data, errors = [], now: fixed }: { data: SceneData; errors?: string[]; now?: number }) {
  const clock = useClock(fixed)
  const minute = clock - (clock % 60_000)
  const field = useRef<HTMLDivElement>(null)
  const hv = useFieldHeight(field)
  const m = useMemo(() => scene(data, minute, hv), [data, minute, hv])
  const canvas = useRef<HTMLCanvasElement>(null)
  useField(canvas, m.hairlines, hv)
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
          {need > 0 && <rect x={1852} y={9} width={3} height={10} fill={C.accent} />}
          <text x={1896} y={18} fontSize={10} fill={C.secondary} textAnchor="end">
            ● {hhmmss(clock)}
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

          {/* (c) coloured flows, (d) their needs-you dots */}
          {m.flows.map((fl) => {
            if (fl.kind === 'due') {
              // Revision 1: a due-later routine's faint connector — ring → its hollow square, nothing else.
              return (
                <To key={fl.id} target={fl.link} title={fl.title} data-flow={fl.id} className="hud-flow-in">
                  <path d={fl.d} stroke={fl.color} strokeOpacity={0.5} strokeWidth={1.1} strokeDasharray="3 3" fill="none" />
                </To>
              )
            }
            const need = fl.kind === 'need' ? m.needs.find((n) => `need:${n.item.id}` === fl.id) : undefined
            return (
              <To key={fl.id} target={fl.link} title={fl.title} data-flow={fl.id} className="hud-flow-in">
                <path d={fl.d} stroke={fl.color} strokeOpacity={0.95} strokeWidth={1.1} fill="none" />
                <circle cx={fl.end[0]} cy={fl.end[1]} r={1.8} fill={fl.color} />
                <text x={fl.end[0] + 9} y={fl.end[1] + 3.5} fontSize={10} fill={fl.color}>
                  {fl.label}
                </text>
                {need && (
                  <>
                    <circle cx={need.x} cy={need.y} r={7} fill={C.page} stroke={C.accent} strokeWidth={1.2} />
                    <circle cx={need.x} cy={need.y} r={1.6} fill={C.accent} />
                    <text x={need.x - 13} y={need.y + 4} fontSize={10} fill={C.accent} textAnchor="end">
                      {need.label}
                    </text>
                  </>
                )}
              </To>
            )
          })}
          {m.flowsMore > 0 && (
            <text x={1500} y={52} fontSize={9} fill={C.tertiary}>
              +{m.flowsMore} more
            </text>
          )}

          {/* (e) rings, (f) their labels, (g) Revision 2: the desk's context gauge on the outer track — length-coded only, faience when live, else the secondary ink;
              (h) Revision 4: a root session's live subagents as satellites on r 23 from 3 o'clock clockwise, 8 then `+N` */}
          {m.rings.map((r) => {
            const sats = r.children ?? []
            const lx = r.x - (sats.length > 0 ? 30 : 22)
            return (
              <To key={r.id} target={r.link} title={`${r.name} · ${r.sub || r.style}${r.ctx ? ` · ${ctxTitle(r.ctx)}` : ''}`}>
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
                <RingGlyph r={r} x={r.x} y={r.y} />
                <text x={lx} y={r.y - 1} fontSize={11} fill={r.style === 'lapsed' ? C.tertiary : C.primary} textAnchor="end">
                  {r.name}
                </text>
                <text x={lx} y={r.y + 12} fontSize={9} fill={ringColor(r.style)} textAnchor="end">
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
