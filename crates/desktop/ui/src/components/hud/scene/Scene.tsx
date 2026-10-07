import { Link } from '@tanstack/react-router'
import { type ReactNode, type RefObject, type SVGProps, useEffect, useMemo, useRef, useState } from 'react'
import { age } from '@/components/tasks/helpers'
import { spend } from '../model'
import { C, type Color, H, hhmm, hhmmss, MIX_ORDER, type Hairline, type OutcomeMix, type PunchCard, type Ring, type SceneData, type SceneModel, type Target, actorLabel, ringColor, ringDashed, ringDots, scene, streamColor, truncate, W } from './model'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'

/**
 * 128 HUD-SCENE: the visual overview on `/` (≥ md). One 1920×1080 SVG scaled to its container over a canvas that
 * carries the grey hairline field; every coloured mark decodes to a fact and links to its screen.
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

/** The hairline field on a canvas under the SVG: redrawn on data change and at most once a second; the colour is read once per theme change. */
function useField(ref: RefObject<HTMLCanvasElement | null>, lines: readonly Hairline[]) {
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
      if (c.width !== W * dpr) {
        c.width = W * dpr
        c.height = H * dpr
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, W, H)
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

/** A `<g>` with a tooltip whose content links to `target` (a TanStack `Link`, which renders an SVG `<a>` here). */
export function To({ target, title, children, ...g }: { target: Target | null; title: string; children: ReactNode } & Omit<SVGProps<SVGGElement>, 'target' | 'title'>) {
  let a: ReactNode = children
  if (target !== null) {
    switch (target.route) {
      case 'attention':
        a = <Link to="/attention/$id" params={{ id: target.id }}>{children}</Link>
        break
      case 'task':
        a = <Link to="/tasks/$id" params={{ id: target.id }}>{children}</Link>
        break
      case 'agent':
        a = <Link to="/agents/$name" params={{ name: target.name }}>{children}</Link>
        break
      case 'routine':
        a = <Link to="/routines/$name" params={{ name: target.name }}>{children}</Link>
        break
      case 'session':
        a = <Link to="/c/$session" params={{ session: target.id }}>{children}</Link>
        break
      case 'agents':
        a = <Link to="/agents">{children}</Link>
        break
      case 'tasks-doing':
        a = <Link to="/tasks" search={{ state: 'doing', project: '' }}>{children}</Link>
        break
      case 'attention-list':
        a = <Link to="/attention">{children}</Link>
        break
    }
  }
  return (
    <g {...g}>
      <title>{title}</title>
      {a}
    </g>
  )
}

// --------------------------------------------------------------------------------------------------------------- panels

/** One bottom-band panel in local 352×248 coordinates; `SceneCompact` draws each in its own SVG. */
export function Panel({ title, meta, big, children }: { title: string; meta: string; big: string; children: ReactNode }) {
  return (
    <g>
      <rect width={352} height={248} fill={C.elevated} stroke={C.border} />
      <rect x={12} y={13} width={5} height={5} fill={C.tertiary} />
      <text x={24} y={19} fontSize={9} fill={C.primary} style={LS}>{title}</text>
      <text x={340} y={19} fontSize={8} fill={C.tertiary} textAnchor="end" style={{ letterSpacing: 1 }}>{meta}</text>
      <line x1={12} y1={28} x2={340} y2={28} stroke={C.border} />
      {children}
      <text x={340} y={236} fontSize={18} fill={C.borderStrong} textAnchor="end">{big}</text>
    </g>
  )
}

export function AgentsPanel({ rings, activityToday }: { rings: readonly Ring[]; activityToday: readonly ActivityEntry[] }) {
  const live = rings.filter((r) => r.style === 'live').length
  return (
    <Panel title="AGENTS" meta={`${live} LIVE`} big={`${live} live`}>
      {rings.slice(0, 6).map((r, i) => {
        const y = 48 + 32 * i
        return (
          <To key={r.id} target={r.link} title={`${r.name} · ${r.sub}`}>
            <text x={14} y={y} fontSize={10} fill={r.style === 'lapsed' ? C.tertiary : C.primary}>{r.name}</text>
            <text x={340} y={y} fontSize={9} fill={C.secondary} textAnchor="end">{r.sub}</text>
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
          <text x={14} y={55 + 24 * d} fontSize={9} fill={C.tertiary}>{DAYS[d]}</text>
          {row.map((n, h) => {
            const v = punch.max > 0 ? n / punch.max : 0
            return <circle key={h} cx={34 + 13.2 * h} cy={52 + 24 * d} r={0.6 + 2.6 * v} fill={C.secondary} fillOpacity={0.25 + 0.75 * v} />
          })}
        </g>
      ))}
      <circle cx={34 + 13.2 * nh} cy={52 + 24 * nd} r={6} fill="none" stroke={C.primary} strokeWidth={1} />
      {[0, 6, 12, 18].map((h) => (
        <text key={h} x={34 + 13.2 * h} y={226} fontSize={8} fill={C.tertiary} textAnchor="middle">{pad2(h)}</text>
      ))}
    </Panel>
  )
}

export function OutcomesPanel({ mix }: { mix: OutcomeMix }) {
  const total = MIX_ORDER.reduce((s, m) => s + mix[m.key], 0)
  const scale = total > 60 ? 60 / total : 1
  const fills: Color[] = []
  for (const m of MIX_ORDER) for (let i = 0; i < Math.round(mix[m.key] * scale) && fills.length < 60; i++) fills.push(m.color)
  return (
    <Panel title="OUTCOMES" meta={`OF ${total}`} big={String(total)}>
      {Array.from({ length: 60 }, (_, i) => {
        const fill = fills[i]
        return <circle key={i} cx={20 + 14 * (i % 10)} cy={50 + 14 * Math.floor(i / 10)} r={3.2} fill={fill ?? 'none'} stroke={fill ? undefined : C.borderStrong} strokeWidth={1} />
      })}
      {MIX_ORDER.map((m, k) => (
        <g key={m.key}>
          <circle cx={186} cy={49 + 18 * k} r={2.5} fill={m.color} />
          <text x={196} y={52 + 18 * k} fontSize={10} fill={C.secondary}>{m.key}</text>
          <text x={338} y={52 + 18 * k} fontSize={10} fill={C.primary} textAnchor="end">{mix[m.key]}</text>
        </g>
      ))}
    </Panel>
  )
}

export function SpendPanel({ agents }: { agents: readonly AgentSummary[] }) {
  const rows = spend(agents)
  const total = agents.reduce((s, a) => s + a.spent_month, 0)
  return (
    <Panel title="SPEND" meta="MONTH" big={`$${total.toFixed(2)}`}>
      {rows.slice(0, 5).map((r, i) => {
        const y = 50 + 32 * i
        return (
          <To key={r.name} target={{ route: 'agent', name: r.name }} title={`${r.name} · $${r.spent.toFixed(2)}${r.cap === null ? '' : ` of $${r.cap}`} this month`}>
            <text x={14} y={y} fontSize={10} fill={C.primary}>{r.name}</text>
            <text x={340} y={y} fontSize={9} fill={C.secondary} textAnchor="end">{r.cap === null ? `$${r.spent.toFixed(1)} · no cap` : `$${r.spent.toFixed(1)} / $${r.cap}`}</text>
            <line x1={14} y1={y + 9} x2={338} y2={y + 9} stroke={C.borderStrong} strokeWidth={2} />
            {r.share !== null && r.share > 0 && <line x1={14} y1={y + 9} x2={14 + 324 * r.share} y2={y + 9} stroke={r.share > 0.85 ? C.danger : C.success} strokeWidth={2} />}
          </To>
        )
      })}
    </Panel>
  )
}

export function StreamPanel({ activity }: { activity: readonly ActivityEntry[] }) {
  return (
    <Panel title="STREAM" meta="LIVE" big={`${activity.length}/50`}>
      {activity.slice(0, 7).map((e, i) => {
        const y = 50 + 20 * i
        const color = streamColor(e.kind)
        return (
          <g key={e.id}>
            <title>{`${hhmmss(e.ts_ms)} · ${actorLabel(e.actor)} · ${e.summary}`}</title>
            <text x={14} y={y} fontSize={9} fill={C.tertiary}>{hhmmss(e.ts_ms)}</text>
            <text x={76} y={y} fontSize={9} fill={color}>{actorLabel(e.actor)}</text>
            <text x={160} y={y} fontSize={9} fill={color}>{truncate(e.summary, 30)}</text>
          </g>
        )
      })}
    </Panel>
  )
}

// ---------------------------------------------------------------------------------------------------------------- scene

export const PANEL_X = [24, 404, 784, 1164, 1544] as const

/** The ring glyph at (x, y): filled + pulsing when live, hollow when idle, gold when waiting, dashed when lapsed/error. */
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

export function footerSegments(m: SceneModel): { text: string; color: string; target: Target | null }[] {
  const f = m.footer
  return [
    { text: `${f.sessionsLive} SESSIONS LIVE`, color: C.tertiary, target: null },
    { text: `${f.agentsWorking} AGENTS WORKING`, color: C.tertiary, target: { route: 'agents' } },
    { text: `${f.needYou === null ? '—' : pad2(f.needYou)} NEED YOU`, color: f.needYou !== null && f.needYou > 0 ? C.accent : C.tertiary, target: { route: 'attention-list' } },
    { text: `$${f.month.toFixed(2)} THIS MONTH`, color: C.tertiary, target: { route: 'agents' } },
    { text: `${f.outcomes} OUTCOMES TODAY · ${f.inFlight} IN FLIGHT`, color: C.tertiary, target: { route: 'tasks-doing' } },
  ]
}

export function Scene({ data, errors = [], now: fixed }: { data: SceneData; errors?: string[]; now?: number }) {
  const clock = useClock(fixed)
  const minute = clock - (clock % 60_000)
  const m = useMemo(() => scene(data, minute), [data, minute])
  const canvas = useRef<HTMLCanvasElement>(null)
  useField(canvas, m.hairlines)
  const f = m.footer
  const need = f.needYou ?? 0
  const summary = `${f.needYou ?? '—'} need you · ${f.agentsWorking} agents working · ${f.sessionsLive} sessions live · ${f.outcomes} outcomes today · ${f.inFlight} in flight · $${f.month.toFixed(2)} this month`
  const segs = footerSegments(m)
  const segX = [28, 190, 356, 470]
  return (
    <div>
      <div className="hud-scene relative w-full" style={{ aspectRatio: '1920 / 1080', minHeight: 540 }}>
        <canvas ref={canvas} width={W} height={H} aria-hidden className="absolute inset-0 h-full w-full object-contain" />
        <svg viewBox={`0 0 ${W} ${H}`} className="relative block h-full w-full font-mono" role="img" aria-labelledby="hud-scene-title hud-scene-desc">
          <title id="hud-scene-title">Overview</title>
          <desc id="hud-scene-desc">{summary}</desc>

          {/* top bar */}
          <text x={24} y={18} fontSize={10} fill={C.accent} style={{ letterSpacing: 2 }}>T H E U T H</text>
          <line x1={1660} y1={14} x2={1840} y2={14} stroke={C.borderStrong} strokeDasharray="2 4" />
          {need > 0 && <rect x={1852} y={9} width={3} height={10} fill={C.accent} />}
          <text x={1896} y={18} fontSize={10} fill={C.secondary} textAnchor="end">● {hhmmss(clock)}</text>
          {[
            [14, 54, 14, 40, 28, 40],
            [1906, 54, 1906, 40, 1892, 40],
            [14, 758, 14, 772, 28, 772],
            [1906, 758, 1906, 772, 1892, 772],
          ].map((p, i) => (
            <path key={i} d={`M ${p[0]} ${p[1]} L ${p[2]} ${p[3]} L ${p[4]} ${p[5]}`} stroke={C.borderStrong} fill="none" />
          ))}

          {/* (b) the day arc */}
          <path d={m.arc.band} fill={C.elevated} stroke={C.borderStrong} />
          {m.arc.ticks.map((t, i) => (
            <line key={i} x1={t.x1} y1={t.y1} x2={t.x2} y2={t.y2} stroke={C.tertiary} />
          ))}
          {m.arc.labels.map((l) => (
            <text key={l.text} x={l.x} y={l.y} fontSize={8} fill={C.tertiary} textAnchor="middle" transform={`rotate(${l.rot.toFixed(1)} ${l.x.toFixed(1)} ${l.y.toFixed(1)})`}>{l.text}</text>
          ))}
          {m.arc.marks.map((k, i) => (
            <To key={`${k.name}:${i}`} target={k.link} title={`${k.name} · ${k.filled ? 'fired today' : 'due later today'}`}>
              <rect x={k.x - 2.5} y={k.y - 2.5} width={5} height={5} fill={k.filled ? C.primary : 'none'} stroke={C.primary} strokeWidth={0.8} transform={`rotate(${k.rot.toFixed(1)} ${k.x.toFixed(1)} ${k.y.toFixed(1)})`} />
              <text x={k.lx} y={k.ly} fontSize={7} fill={k.filled ? C.secondary : C.tertiary} textAnchor="middle" transform={`rotate(${k.lrot.toFixed(1)} ${k.lx.toFixed(1)} ${k.ly.toFixed(1)})`}>{k.name}</text>
            </To>
          ))}
          <line x1={m.arc.now.x1} y1={m.arc.now.y1} x2={m.arc.now.x2} y2={m.arc.now.y2} stroke={C.primary} strokeWidth={1.5} />
          <text x={m.arc.now.lx} y={m.arc.now.ly} fontSize={9} fill={C.primary} transform={`rotate(${m.arc.now.lrot.toFixed(1)} ${m.arc.now.lx.toFixed(1)} ${m.arc.now.ly.toFixed(1)})`}>now</text>

          {/* (c) coloured flows, (d) their needs-you dots */}
          {m.flows.map((fl) => {
            const need = fl.kind === 'need' ? m.needs.find((n) => `need:${n.item.id}` === fl.id) : undefined
            return (
              <To key={fl.id} target={fl.link} title={fl.title} data-flow={fl.id} className="hud-flow-in">
                <path d={fl.d} stroke={fl.color} strokeOpacity={0.95} strokeWidth={1.1} fill="none" />
                <circle cx={fl.end[0]} cy={fl.end[1]} r={1.8} fill={fl.color} />
                <text x={fl.end[0] + 9} y={fl.end[1] + 3.5} fontSize={10} fill={fl.color}>{fl.label}</text>
                {need && (
                  <>
                    <circle cx={need.x} cy={need.y} r={7} fill={C.page} stroke={C.accent} strokeWidth={1.2} />
                    <circle cx={need.x} cy={need.y} r={1.6} fill={C.accent} />
                    <text x={need.x - 13} y={need.y + 4} fontSize={10} fill={C.accent} textAnchor="end">{need.label}</text>
                  </>
                )}
              </To>
            )
          })}
          {m.flowsMore > 0 && <text x={1500} y={52} fontSize={9} fill={C.tertiary}>+{m.flowsMore} more</text>}

          {/* (e) rings, (f) their labels */}
          {m.rings.map((r) => (
            <To key={r.id} target={r.link} title={`${r.name} · ${r.sub || r.style}`}>
              <RingGlyph r={r} x={r.x} y={r.y} />
              <text x={r.x - 22} y={r.y - 1} fontSize={11} fill={r.style === 'lapsed' ? C.tertiary : C.primary} textAnchor="end">{r.name}</text>
              <text x={r.x - 22} y={r.y + 12} fontSize={9} fill={ringColor(r.style)} textAnchor="end">{r.sub}</text>
            </To>
          ))}

          {/* NEEDS YOU table */}
          <text x={28} y={60} fontSize={9} fill={C.tertiary} style={LS}>NEEDS YOU</text>
          <text x={430} y={60} fontSize={9} fill={need > 0 ? C.accent : C.tertiary} textAnchor="end">{f.needYou === null ? '—' : pad2(f.needYou)}</text>
          <line x1={28} y1={66} x2={430} y2={66} stroke={C.border} />
          {(['ID', 'KIND', 'AGE', 'TITLE'] as const).map((h, i) => (
            <text key={h} x={[28, 90, 160, 200][i]} y={82} fontSize={9} fill={C.tertiary}>{h}</text>
          ))}
          {f.needYou === 0 && <text x={28} y={100} fontSize={11} fill={C.tertiary}>Nothing needs you</text>}
          {m.needs.slice(0, 6).map((n, i) => (
            <To key={n.item.id} target={n.link} title={`${n.item.kind} · ${n.item.title}${n.item.detail ? ` — ${n.item.detail}` : ''}`}>
              <text x={28} y={100 + 17 * i} fontSize={11} fill={C.accent}>{n.label}</text>
              <text x={90} y={100 + 17 * i} fontSize={11} fill={C.accent}>{n.item.kind}</text>
              <text x={160} y={100 + 17 * i} fontSize={11} fill={C.accent}>{age(n.item.at_ms, minute)}</text>
              <text x={200} y={100 + 17 * i} fontSize={11} fill={C.accent}>{truncate(n.item.title, 38)}</text>
            </To>
          ))}
          {need > 6 && (
            <To target={{ route: 'attention-list' }} title={`${need - 6} more in the inbox`}>
              <text x={28} y={100 + 17 * 6} fontSize={11} fill={C.accent}>+{need - 6}</text>
            </To>
          )}

          {/* footer */}
          <line x1={24} y1={778} x2={1896} y2={778} stroke={C.border} />
          {segs.map((s, i) => (
            <To key={s.text} target={s.target} title={s.text}>
              <text x={i === 4 ? 1896 : segX[i]} y={795} fontSize={9} fill={s.color} textAnchor={i === 4 ? 'end' : 'start'} style={LS}>{s.text}</text>
            </To>
          ))}

          {/* bottom band */}
          <g transform={`translate(${PANEL_X[0]} 812)`}><AgentsPanel rings={m.rings} activityToday={data.activityToday ?? []} /></g>
          <g transform={`translate(${PANEL_X[1]} 812)`}><ArrivalsPanel punch={m.punch} now={clock} /></g>
          <g transform={`translate(${PANEL_X[2]} 812)`}><OutcomesPanel mix={m.mix} /></g>
          <g transform={`translate(${PANEL_X[3]} 812)`}><SpendPanel agents={data.agents ?? []} /></g>
          <g transform={`translate(${PANEL_X[4]} 812)`}><StreamPanel activity={data.activity ?? []} /></g>
        </svg>
      </div>
      {errors.length > 0 && <p className="mt-1 font-mono text-xs" style={{ color: C.danger }}>{errors.join(' · ')} failed to load</p>}
    </div>
  )
}
