// 218 §8.8 "Reply bubble" / design §4.5: adapter over the shared `ReplyBubble` for whiteboards (pinned to a card or
// floating above the composer dock, with turn undo).

import { ReplyBubble } from '@/components/chat/floating/ReplyBubble'

export type BubbleProps = {
  text: string
  /** Undo this turn is offered (the turn wrote to this whiteboard and is not undone). */
  canUndo: boolean
  undoing: boolean
  onUndo(): void
  onDismiss(): void
  onOpenSession(): void
  /** Above the pill (not pinned to a card); `beside` moves it left of a raised cluster. */
  floating: boolean
  beside?: boolean
}

export function Bubble(p: BubbleProps) {
  return <ReplyBubble {...p} />
}
