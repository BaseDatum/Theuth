import { type ReactNode, useMemo } from 'react'
import { age } from '@/components/tasks/helpers'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { C, type SceneData, type Target, ringColor, scene, truncate } from './model'
import { footerSegments, linkFor, Panels, RingGlyph, Scene } from './Scene'

/**
 * 128 HUD-SCENE under `md`: no flow field and no arc (they need width). Footer pills, the NEEDS YOU rows, the rings as
 * a list, then the five panels stacked at container width — every destination of the desktop scene is one tap here too.
 */

function Row({ target, className, children }: { target: Target | null; className: string; children: ReactNode }) {
  return target === null ? <div className={className}>{children}</div> : linkFor(target, children, cn(className, 'hover:bg-accent'))
}

type SceneProps = { data: SceneData; errors?: string[]; now?: number }

/** `Scene` from `md` up, `SceneCompact` below — the app's one breakpoint (`useIsMobile`, Tailwind's `md:` negated). */
export function SceneSwitch(props: SceneProps) {
  return useIsMobile() ? <SceneCompact {...props} /> : <Scene {...props} />
}

export function SceneCompact({ data, errors = [], now: fixed }: SceneProps) {
  const now = fixed ?? Date.now()
  const m = useMemo(() => scene(data, now), [data, now])
  return (
    <div className="flex flex-col gap-3 font-mono">
      <div className="flex flex-wrap gap-1.5">
        {footerSegments(m).map((s) => (
          <Row key={s.text} target={s.target} className={cn('rounded-full border border-border-subtle px-2.5 py-0.5 text-[9px] tracking-[1.5px]')}>
            <span style={{ color: s.color }}>{s.text}</span>
          </Row>
        ))}
      </div>
      <section aria-label="Needs you">
        {linkFor({ route: 'attention-list' }, <h3 className="text-[9px] tracking-[1.5px] hover:underline" style={{ color: C.tertiary }}>NEEDS YOU</h3>)}
        {m.footer.needYou === 0 && (
          <p className="py-1 text-[11px]" style={{ color: C.tertiary }}>
            Nothing needs you
          </p>
        )}
        <ul>
          {m.needs.map((n) => (
            <li key={n.item.id}>
              <Row target={n.link} className="flex gap-3 py-1 text-[11px]">
                <span className="w-16 shrink-0 truncate" style={{ color: C.accent }}>
                  {n.label}
                </span>
                <span className="w-14 shrink-0" style={{ color: C.accent }}>
                  {n.item.kind}
                </span>
                <span className="w-8 shrink-0" style={{ color: C.accent }}>
                  {age(n.item.at_ms, now)}
                </span>
                <span className="min-w-0 truncate" style={{ color: C.accent }}>
                  {truncate(n.item.title, 38)}
                </span>
              </Row>
            </li>
          ))}
        </ul>
      </section>
      <ul aria-label="Rings">
        {m.rings.map((r) => (
          <li key={r.id}>
            <Row target={r.link} className="flex items-center gap-3 px-1 py-1.5">
              <svg viewBox="0 0 32 32" width={32} height={32} aria-hidden>
                <RingGlyph r={r} x={16} y={16} />
              </svg>
              <span className="text-[11px]" style={{ color: r.style === 'lapsed' ? C.tertiary : C.primary }}>
                {r.name}
              </span>
              <span className="ml-auto text-[9px]" style={{ color: ringColor(r.style) }}>
                {r.sub}
              </span>
            </Row>
          </li>
        ))}
      </ul>
      <Panels data={data} m={m} now={now} stream="max-h-[40vh]" />
      {errors.length > 0 && (
        <p className="text-xs" style={{ color: C.danger }}>
          {errors.join(' · ')} failed to load
        </p>
      )}
    </div>
  )
}
