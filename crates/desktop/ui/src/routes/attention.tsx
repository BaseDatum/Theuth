import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/attention/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { Page } from '@/components/attention/Page'

/** The attention inbox as master–detail (101 D6): the rail here, the selected item's screen in the outlet. The sidebar row's badge is its count (70 HUD). */
export const Route = createFileRoute('/attention')({
  // The detail pane can also hold a live chat (`?chat=<session>`) or a new-session draft (`?new=true`). Tapping an
  // item clears both, so only an explicit choice pins the pane; live items never replace a chat or a draft.
  validateSearch: (s: Record<string, unknown>): { chat?: string; new?: true } => ({ chat: typeof s.chat === 'string' && s.chat !== '' ? s.chat : undefined, new: s.new === true ? true : undefined }),
  component: withCapability(Page, requirements),
})
