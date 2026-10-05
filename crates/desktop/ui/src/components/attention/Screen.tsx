import { Link } from '@tanstack/react-router'
import { ChevronLeft } from 'lucide-react'
import type { ComponentType } from 'react'
import { KIND_LABEL } from '@/components/hud/Hud'
import { Button } from '@/components/ui/button'
import { isMacDesktop } from '@/lib/hotkeys'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import { ApprovalScreen } from './ApprovalScreen'
import { AskScreen } from './AskScreen'
import { DecisionScreen } from './DecisionScreen'
import { BlockedScreen, BudgetScreen, HandoffScreen, HireScreen, NodeScreen, SecretScreen } from './PlainScreens'
import { ReviewScreen } from './ReviewScreen'
import { ShowScreen } from './ShowScreen'

const TITLE: Partial<Record<AttentionItem['kind'], string>> = { decision: 'Decision', review: 'Review', ask: 'Question', show: 'Shown to you', approval: 'Approval' }

/** 101 D7 / 104 A6: one screen per kind. */
const SCREEN: Record<AttentionItem['kind'], ComponentType<{ item: AttentionItem }>> = {
  decision: DecisionScreen,
  review: ReviewScreen,
  ask: AskScreen,
  show: ShowScreen,
  approval: ApprovalScreen,
  handoff: HandoffScreen,
  secret: SecretScreen,
  hire: HireScreen,
  budget: BudgetScreen,
  node: NodeScreen,
  blocked: BlockedScreen,
}

/** The selected item's screen (101 D7, 104 A6): every kind has one. Under `md` a 52 px header with the back chevron. */
export function Screen({ item }: { item: AttentionItem }) {
  const Kind = SCREEN[item.kind]
  return (
    <>
      <header data-tauri-drag-region={isMacDesktop() || undefined} className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3 md:hidden">
        <Button asChild variant="ghost" size="icon-sm" aria-label="Back to attention">
          <Link to="/attention" search={true}>
            <ChevronLeft />
          </Link>
        </Button>
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{TITLE[item.kind] ?? KIND_LABEL[item.kind]}</h2>
      </header>
      <Kind key={item.id} item={item} />
    </>
  )
}
