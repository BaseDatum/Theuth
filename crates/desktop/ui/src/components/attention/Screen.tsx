import { Link } from '@tanstack/react-router'
import { ChevronLeft } from 'lucide-react'
import { KIND_LABEL, KindActions } from '@/components/hud/Hud'
import { age } from '@/components/tasks/helpers'
import { Button } from '@/components/ui/button'
import { isMacDesktop } from '@/lib/hotkeys'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import { DecisionScreen } from './DecisionScreen'
import { ReviewScreen } from './ReviewScreen'
import { Body, Chip, useResolved } from './shared'

const TITLE: Partial<Record<AttentionItem['kind'], string>> = { decision: 'Decision', review: 'Review' }

/** The selected item's screen (101 D7): decision and review get theirs, every other kind the plain one. Under `md` a 52 px header with the back chevron. */
export function Screen({ item }: { item: AttentionItem }) {
  return (
    <>
      <header data-tauri-drag-region={isMacDesktop() || undefined} className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3 md:hidden">
        <Button asChild variant="ghost" size="icon-sm" aria-label="Back to attention">
          <Link to="/attention">
            <ChevronLeft />
          </Link>
        </Button>
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{TITLE[item.kind] ?? KIND_LABEL[item.kind]}</h2>
      </header>
      {item.kind === 'decision' ? <DecisionScreen item={item} /> : item.kind === 'review' ? <ReviewScreen item={item} /> : <PlainScreen item={item} />}
    </>
  )
}

/** D7: approval, hire, blocked, budget, node, handoff, secret — title, detail, the card's buttons at `sm`. */
function PlainScreen({ item }: { item: AttentionItem }) {
  const onResolved = useResolved(item)
  return (
    <Body>
      <div className="flex flex-col gap-2">
        <div className="flex items-center gap-2 text-xs text-fg-tertiary">
          <Chip kind={item.kind} />
          {item.agent && <span>by {item.agent}</span>}
          <span className="ml-auto tabular-nums">{age(item.at_ms)}</span>
        </div>
        <h1 className="text-xl font-semibold leading-snug">{item.title}</h1>
        {item.detail && <p className="text-sm text-fg-secondary">{item.detail}</p>}
      </div>
      <KindActions item={item} size="sm" onResolved={onResolved} />
      {item.task && (
        <Button asChild variant="outline" size="sm" className="w-fit">
          <Link to="/tasks/$id" params={{ id: item.task }}>
            Open task
          </Link>
        </Button>
      )}
    </Body>
  )
}
