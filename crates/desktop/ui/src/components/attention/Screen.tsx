import { Link } from '@tanstack/react-router'
import { ChevronLeft } from 'lucide-react'
import type { ComponentType } from 'react'
import { itemLabel } from '@/components/hud/Hud'
import { Button } from '@/components/ui/button'
import { isMacDesktop } from '@/lib/hotkeys'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import { ApprovalScreen } from './ApprovalScreen'
import { AskScreen } from './AskScreen'
import { DecisionScreen } from './DecisionScreen'
import { BlockedScreen, BudgetScreen, ContactFirstScreen, DoubleScreen, FinishedScreen, HandoffScreen, HireScreen, HomeScreen, NodeScreen, ResumeScreen, SecretScreen } from './PlainScreens'
import { ReviewScreen } from './ReviewScreen'
import { ShowScreen } from './ShowScreen'

const TITLE: Partial<Record<AttentionItem['kind'], string>> = { decision: 'Decision', review: 'Review', ask: 'Question', show: 'Shown to you', approval: 'Approval', double: 'Double', contact_first: 'First contact' }

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
  home: HomeScreen,
  blocked: BlockedScreen,
  resume: ResumeScreen,
  double: DoubleScreen,
  finished: FinishedScreen,
  contact_first: ContactFirstScreen,
}

/** The selected item's screen (101 D7, 104 A6): every kind has one. Under `md` a 52 px header with the back chevron. */
export function Screen({ item }: { item: AttentionItem }) {
  // 183 review 1: a kind a newer engine sends (rail group Other) shows its title and detail.
  const Kind = SCREEN[item.kind] ?? HomeScreen
  return (
    <>
      <header data-tauri-drag-region={isMacDesktop() || undefined} className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3 md:hidden">
        <Button asChild variant="ghost" size="icon-sm" aria-label="Back to attention">
          <Link to="/attention" search={true}>
            <ChevronLeft />
          </Link>
        </Button>
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{TITLE[item.kind] ?? itemLabel(item)}</h2>
      </header>
      <Kind key={item.id} item={item} />
    </>
  )
}
