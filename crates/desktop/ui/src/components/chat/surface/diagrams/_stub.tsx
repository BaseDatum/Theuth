// 123 step 0: the placeholder every new diagram component mounts until its wave lands the real `<NameSvg>` /
// `<NameView>` (design 123 §5.2). `DiagramFrame` around an accessible `<svg>` carrying the title and the one-line
// summary (`6 tasks`) as text — the same words the TUI's placeholder arm prints. The closer deletes this file once
// no `diagrams/*.tsx` imports it.
import { useId, useMemo, useSyncExternalStore } from 'react'
import { countOf, describe, svgA11y } from '@/lib/diagram/a11y'
import { diagramTokens } from '@/lib/diagram-theme'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from '../../DiagramFrame'

export function DiagramStub({ name, title, n, unit, interactive }: { name: string; title?: string | null; n: number; unit: string; interactive?: boolean }) {
  const id = useId().replace(/:/g, '')
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'), () => false)
  const t = useMemo(diagramTokens, [dark])
  const a = svgA11y(`${name.toLowerCase()}-${id}`, !!interactive)
  const summary = countOf(n, unit)
  const W = 320
  const H = title ? 72 : 48
  return (
    <DiagramFrame name={`${name.toLowerCase()}-${id}`} label={name.toLowerCase()} kind={`${name.toLowerCase()}-block`}>
      <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} data-stub={name}>
        <title id={a.titleId}>{title ?? name}</title>
        <desc id={a.descId}>{describe(name, title, n, unit)}</desc>
        <rect x={0.5} y={0.5} width={W - 1} height={H - 1} rx={8} fill={t.card} stroke={t.border} strokeDasharray="4 3" />
        {title && (
          <text x={12} y={24} fontFamily={t.font} fontSize={13} fontWeight={600} fill={t.foreground}>
            {title}
          </text>
        )}
        <text x={12} y={title ? 50 : 29} fontFamily={t.font} fontSize={12} fill={t.mutedForeground}>
          {name} · {summary}
        </text>
      </svg>
    </DiagramFrame>
  )
}
