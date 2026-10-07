// 123 (W3 owns this file): the `Kanban` renderer, moved out of `registry.tsx` unchanged in step 0. W3 adds `limit`,
// `state`, `rows`/`row`/`cut` and `title` here (design 123 §4.1); keep the exported `Kanban` name — `registry.tsx`
// mounts it.
import type { z } from 'zod'
import type { catalog } from '../catalog'
import { Tag } from '../Tag'

export type KanbanData = z.infer<(typeof catalog)['data']['components']['Kanban']['props']>

export function Kanban({ props }: { props: KanbanData }) {
  return (
    <div className="flex gap-3 overflow-x-auto text-sm">
      {props.columns.map((col, i) => (
        <div key={i} className="flex min-w-48 flex-1 flex-col gap-2 rounded-xl bg-layer-2 p-2">
          <div className="flex items-baseline justify-between px-1">
            <span className="font-medium">{col.title}</span>
            <span className="text-xs text-fg-tertiary tabular-nums">{col.cards.length}</span>
          </div>
          {col.cards.map((c, j) => (
            <div key={j} className="flex flex-col items-start gap-1 rounded-lg border border-border-subtle bg-layer-1 p-2">
              <div>{c.title}</div>
              {c.detail && <div className="text-xs text-fg-secondary">{c.detail}</div>}
              {c.tag && <Tag text={c.tag} variant="secondary" />}
            </div>
          ))}
        </div>
      ))}
    </div>
  )
}
