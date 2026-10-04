// 96 §6.6 / CARD: the voice key's `[screen …]` line as a card under the user's words — what the pointer did on that
// display in plain words, each gesture's accessibility elements as chips, the stamped overview and crops as
// thumbnails. The same card family as the harness-injected cards (`HarnessCards.tsx`); coordinates live in tooltips.

import { Circle, type LucideIcon, MousePointer2, ScanEye, Timer, Underline } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import type { Element, Gesture, Screen } from '@/lib/screen-context'
import type { ContextAttachment } from '@/rpc/bindings/ContextAttachment'
import { Attachments } from './Attachments'

const ICON: Record<Gesture['kind'], LucideIcon> = { circled: Circle, underlined: Underline, rested: Timer, ended: MousePointer2 }
const CHIP = 'inline-flex max-w-full min-w-0 items-center gap-1 rounded-full bg-layer-2 px-2 leading-5'

const point = (p: [number, number]) => `(${p[0]},${p[1]})`

/** "Circled", "Underlined", "Rested 1.8 s on", "Ended on"; the trailing `on` goes when there is nothing to be on. */
export function verb(g: Gesture): string {
  const v = g.kind === 'circled' ? 'Circled' : g.kind === 'underlined' ? 'Underlined' : g.kind === 'rested' ? `Rested ${((g.ms ?? 0) / 1000).toFixed(1)} s on` : 'Ended on'
  return g.elements.length === 0 && v.endsWith(' on') ? v.slice(0, -3) : v
}

/** The row's tooltip: the gesture's display-local coordinates (and dwell time), which the row itself does not show. */
export function coords(g: Gesture): string | undefined {
  if (g.from && g.to) return `${point(g.from)}${g.kind === 'circled' ? '–' : '→'}${point(g.to)}`
  if (g.at) return g.ms === undefined ? point(g.at) : `${point(g.at)} · ${g.ms} ms`
  return undefined
}

function ElementChip({ el }: { el: Element }) {
  const body = (
    <>
      <span className="shrink-0 text-fg-tertiary">{el.role}</span>
      {el.label !== undefined && (
        <>
          <span className="shrink-0 text-fg-tertiary">·</span>
          <span className="truncate" title={el.label}>
            {el.label}
          </span>
        </>
      )}
    </>
  )
  return el.url ? (
    <a href={el.url} target="_blank" rel="noopener noreferrer" title={el.url} className={`${CHIP} hover:underline`}>
      {body}
    </a>
  ) : (
    <span className={CHIP}>{body}</span>
  )
}

/** One display's card. `files` are plain image paths (an own send, or `[image: …]` tokens resolved under `attachments/`);
 * `occurrences` are the stored row's attachments once the history is settled, with their removal controls. */
export function ScreenContextCard({
  screen,
  files,
  occurrences,
  session,
  active,
}: {
  screen: Screen
  files?: string[]
  occurrences?: ContextAttachment[]
  session?: string
  active?: boolean
}) {
  const { app, title, url, index, count, gestures, accessibility } = screen
  return (
    <div className="my-2 w-full min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-secondary">
        <ScanEye className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="font-medium">Pointed at the screen</span>
        {app !== undefined && <span className="rounded-full bg-layer-2 px-2 leading-5">{app}</span>}
        {title !== undefined && (
          <span className="max-w-72 min-w-0 truncate" title={title}>
            {title}
          </span>
        )}
        {url !== undefined && (
          <a href={url} target="_blank" rel="noopener noreferrer" title={url} className="max-w-72 min-w-0 truncate hover:underline">
            {url.replace(/^https?:\/\//, '')}
          </a>
        )}
        {count > 1 && <Badge variant="outline">{`${index}/${count}`}</Badge>}
        <Badge variant="outline" className="ml-auto">
          Accessibility {accessibility ? 'on' : 'off'}
        </Badge>
      </div>
      {gestures.length > 0 && (
        <ul className="mt-2 flex flex-col gap-1">
          {gestures.map((g, i) => {
            const Icon = ICON[g.kind]
            return (
              <li key={i} className="flex flex-wrap items-center gap-1.5" title={coords(g)}>
                <Icon className="size-3.5 shrink-0 text-fg-tertiary" aria-hidden="true" />
                <span className="shrink-0">{verb(g)}</span>
                {g.elements.map((el, j) => (
                  <ElementChip key={j} el={el} />
                ))}
              </li>
            )
          })}
        </ul>
      )}
      <Attachments files={files} occurrences={occurrences} session={session} active={active} className="mt-2" />
    </div>
  )
}
