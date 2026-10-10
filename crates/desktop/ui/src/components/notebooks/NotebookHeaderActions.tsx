// 144 §12.1 (wave 2, U2): the notebook header's right group — Ask across notebooks, Overviews ▾, Open in new window
// (Sky's ruling 2026-10-09: a notebook pops out like a note; the app-wide `AppWindow` icon and label), then the
// existing Share. Under 768 px the group is one ⋯ menu.
import { AppWindow, ChevronDown, MessageSquareMore, MoreHorizontal } from 'lucide-react'
import type { ReactNode } from 'react'
import { AskDialog } from '@/components/notebooks/AskDialog'
import { OVERVIEWS, useRunOverview } from '@/components/notebooks/OverviewsSection'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useIsMobile } from '@/hooks/use-mobile'
import { useOverviews } from '@/lib/ask-rpc'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import type { OverviewKind } from '@/rpc/bindings/OverviewKind'

/** The notebook's three columns in their own window (`/notebooks/<id>?window=popout-notebook-<id>`); focus-if-open. */
export const popOutNotebook = (notebook: string): Promise<void> => openPopout({ kind: 'notebook', notebook })

/** `?ask` on the notebook route: `true` = the dialog is open, a string = open with that question typed (`/ask <words>`). */
export type AskSearch = string | true | undefined

export function NotebookHeaderActions({ notebook, ask, onAskChange, share }: { notebook: string; ask: AskSearch; onAskChange(ask: AskSearch): void; share?: ReactNode }) {
  const mobile = useIsMobile()
  const overviews = useOverviews(notebook).data ?? []
  const running = new Set(overviews.filter((o) => o.status === 'running').map((o) => o.kind))
  const run = useRunOverview(notebook)
  const popout = canPopout() && !isPopout()
  const item = (kind: OverviewKind, label: string) => (
    <DropdownMenuItem key={kind} disabled={run.isPending || running.has(kind)} onSelect={() => run.mutate(kind)}>
      <span className="flex-1">{label}</span>
      {running.has(kind) && <span className="text-xs text-fg-tertiary">running…</span>}
    </DropdownMenuItem>
  )
  return (
    <>
      {mobile ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label="Notebook actions">
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => onAskChange(true)}>
              <MessageSquareMore /> Ask across notebooks
            </DropdownMenuItem>
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>Overviews</DropdownMenuSubTrigger>
              <DropdownMenuSubContent>{OVERVIEWS.map((o) => item(o.kind, o.label))}</DropdownMenuSubContent>
            </DropdownMenuSub>
            {popout && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={() => void popOutNotebook(notebook)}>
                  <AppWindow /> Open in new window
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : (
        <>
          <Button variant="outline" size="sm" onClick={() => onAskChange(true)}>
            Ask across notebooks
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm">
                Overviews <ChevronDown />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">{OVERVIEWS.map((o) => item(o.kind, o.label))}</DropdownMenuContent>
          </DropdownMenu>
          {popout && (
            <Button variant="ghost" size="icon-sm" aria-label="Open in new window" title="Open in new window" onClick={() => void popOutNotebook(notebook)}>
              <AppWindow />
            </Button>
          )}
        </>
      )}
      {share}
      <AskDialog open={ask !== undefined} onOpenChange={(o) => !o && onAskChange(undefined)} current={notebook} question={typeof ask === 'string' ? ask : ''} />
    </>
  )
}
