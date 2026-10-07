import { Link } from '@tanstack/react-router'
import { type ReactNode, useMemo } from 'react'
import { age } from '@/components/tasks/helpers'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { C, type SceneData, type Target, ringColor, scene, truncate } from './model'
import { AgentsPanel, ArrivalsPanel, footerSegments, OutcomesPanel, RingGlyph, Scene, SpendPanel, StreamPanel } from './Scene'

/**
 * 128 HUD-SCENE under `md`: no flow field and no arc (they need width). Footer pills, the NEEDS YOU rows, the rings as
 * a list, then the five panels stacked at container width.
 */

function Pill({ target, color, children }: { target: Target | null; color: string; children: ReactNode }) {
  const cls = 'rounded-full border border-border-subtle px-2.5 py-0.5 font-mono text-[9px] tracking-[1.5px]'
  const style = { color }
  if (target?.route === 'agents') return <Link to="/agents" className={cn(cls, 'hover:bg-accent')} style={style}>{children}</Link>
  if (target?.route === 'attention-list') return <Link to="/attention" className={cn(cls, 'hover:bg-accent')} style={style}>{children}</Link>
  if (target?.route === 'tasks-doing') return <Link to="/tasks" search={{ state: 'doing', project: '' }} className={cn(cls, 'hover:bg-accent')} style={style}>{children}</Link>
  return <span className={cls} style={style}>{children}</span>
}

function RingRow({ target, children }: { target: Target | null; children: ReactNode }) {
  const cls = 'flex items-center gap-3 px-1 py-1.5'
  if (target?.route === 'agent') return <Link to="/agents/$name" params={{ name: target.name }} className={cn(cls, 'hover:bg-accent')}>{children}</Link>
  if (target?.route === 'routine') return <Link to="/routines/$name" params={{ name: target.name }} className={cn(cls, 'hover:bg-accent')}>{children}</Link>
  if (target?.route === 'agents') return <Link to="/agents" className={cn(cls, 'hover:bg-accent')}>{children}</Link>
  return <div className={cls}>{children}</div>
}

type SceneProps = { data: SceneData; errors?: string[]; now?: number }

/** `Scene` from `md` up, `SceneCompact` below — the app's one breakpoint (`useIsMobile`, Tailwind's `md:` negated). */
export function SceneSwitch(props: SceneProps) {
  return useIsMobile() ? <SceneCompact {...props} /> : <Scene {...props} />
}

export function SceneCompact({ data, errors = [], now: fixed }: SceneProps) {
  const now = fixed ?? Date.now()
  const m = useMemo(() => scene(data, now), [data, now])
  const panel = (child: ReactNode) => (
    <svg viewBox="0 0 352 248" width="100%" className="block font-mono" role="img">
      {child}
    </svg>
  )
  return (
    <div className="flex flex-col gap-3 font-mono">
      <div className="flex flex-wrap gap-1.5">
        {footerSegments(m).map((s) => (
          <Pill key={s.text} target={s.target} color={s.color}>{s.text}</Pill>
        ))}
      </div>
      <section aria-label="Needs you">
        <h3 className="text-[9px] tracking-[1.5px]" style={{ color: C.tertiary }}>NEEDS YOU</h3>
        {data.attention && m.needs.length === 0 && <p className="py-1 text-[11px]" style={{ color: C.tertiary }}>Nothing needs you</p>}
        <ul>
          {m.needs.map((n) => (
            <li key={n.item.id}>
              <Link to="/attention/$id" params={{ id: n.item.id }} className="flex gap-3 py-1 text-[11px] hover:bg-accent" style={{ color: C.accent }}>
                <span className="w-16 shrink-0 truncate">{n.label}</span>
                <span className="w-14 shrink-0">{n.item.kind}</span>
                <span className="w-8 shrink-0">{age(n.item.at_ms, now)}</span>
                <span className="min-w-0 truncate">{truncate(n.item.title, 38)}</span>
              </Link>
            </li>
          ))}
        </ul>
      </section>
      <ul aria-label="Rings">
        {m.rings.map((r) => (
          <li key={r.id}>
            <RingRow target={r.link}>
              <svg viewBox="0 0 32 32" width={32} height={32} aria-hidden>
                <RingGlyph r={r} x={16} y={16} />
              </svg>
              <span className="text-[11px]" style={{ color: r.style === 'lapsed' ? C.tertiary : C.primary }}>{r.name}</span>
              <span className="ml-auto text-[9px]" style={{ color: ringColor(r.style) }}>{r.sub}</span>
            </RingRow>
          </li>
        ))}
      </ul>
      {panel(<AgentsPanel rings={m.rings} activityToday={data.activityToday ?? []} />)}
      {panel(<OutcomesPanel mix={m.mix} />)}
      {panel(<SpendPanel agents={data.agents ?? []} />)}
      {panel(<ArrivalsPanel punch={m.punch} now={now} />)}
      {panel(<StreamPanel activity={data.activity ?? []} />)}
      {errors.length > 0 && <p className="text-xs" style={{ color: C.danger }}>{errors.join(' · ')} failed to load</p>}
    </div>
  )
}
