// 101: what the three screens share — the D11 "move on" hook, the kind chip, the scrolling body, the footer actions.
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { KIND_LABEL } from '@/components/hud/Hud'
import { snoozeUntil } from '@/components/hud/model'
import { ATTENTION_KEY, useAttentionActions } from '@/components/hud/queries'
import { Button } from '@/components/ui/button'
import { useIsMobile } from '@/hooks/use-mobile'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import { nextAfter } from './model'

/**
 * 101 D11: where the pane goes once `item` left the list — desktop: the next newest item (the list in cache still holds
 * the resolved one at this moment; the refetch is in flight), or `/attention`; phones: back to the list.
 */
export function useResolved(item: AttentionItem): () => void {
  const navigate = useNavigate()
  const qc = useQueryClient()
  const mobile = useIsMobile()
  return () => {
    const next = mobile ? null : nextAfter(qc.getQueryData<AttentionItem[]>(ATTENTION_KEY) ?? [], item.id)
    // `search: true` keeps `?sim=` / `?mock=` alive in dev; in the app the route carries no search.
    void (next ? navigate({ to: '/attention/$id', params: { id: next.id }, search: true }) : navigate({ to: '/attention', search: true }))
  }
}

export function Chip({ kind }: { kind: AttentionItem['kind'] }) {
  return <span className="rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{KIND_LABEL[kind]}</span>
}

/** The scrolling column every screen shares: 720 px of content, padded. */
export function Body({ children }: { children: ReactNode }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto flex w-full max-w-[720px] flex-col gap-6 px-4 py-5 md:px-6">{children}</div>
    </div>
  )
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return <h3 className="text-xs font-semibold tracking-wide text-fg-tertiary uppercase">{children}</h3>
}

/** Open task · Snooze · Dismiss — the footer of the decision and review screens (`onResolved` after Dismiss). */
export function Footer({ item, onResolved, children }: { item: AttentionItem; onResolved: () => void; children?: ReactNode }) {
  const act = useAttentionActions()
  const busy = act.snooze.isPending || act.dismiss.isPending
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {children}
      {item.task && (
        <Button asChild variant="ghost" size="sm">
          <Link to="/tasks/$id" params={{ id: item.task }}>
            Open task
          </Link>
        </Button>
      )}
      <Button variant="ghost" size="sm" disabled={busy} title="Snooze until 09:00 tomorrow" onClick={() => void act.snooze.mutateAsync({ id: item.id, until: snoozeUntil('tomorrow') }).then(onResolved, () => undefined)}>
        Snooze
      </Button>
      <Button variant="ghost" size="sm" disabled={busy} onClick={() => void act.dismiss.mutateAsync(item.id).then(onResolved, () => undefined)}>
        Dismiss
      </Button>
    </div>
  )
}
