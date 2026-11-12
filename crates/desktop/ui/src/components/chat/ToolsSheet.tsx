import { useRouterState } from '@tanstack/react-router'
import { useEffect } from 'react'
import { Tools } from '@/components/library/Tools'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { useToolsSheet } from './tools-sheet'

/** One chat's MCP servers and tools (Settings › Tools › MCP scoped to it): "Off here" and the per-tool switches hide
 * them from this chat and its subagents. Opened from its ⋯ menu (Tools…), `/tools` or `/mcp`. */
export function ToolsSheet({ session }: { session: string }) {
  const open = useToolsSheet((s) => s.session === session)
  const path = useRouterState({ select: (s) => s.location.pathname })
  // Leaving the page closes it: coming back must not reopen it.
  useEffect(
    () => () => {
      const sheet = useToolsSheet.getState()
      if (sheet.session === session) sheet.hide()
    },
    [session, path],
  )
  return (
    <Sheet open={open} onOpenChange={(o) => !o && useToolsSheet.getState().hide()}>
      <SheetContent className="w-full sm:w-[640px] sm:max-w-[640px]">
        <SheetHeader>
          <SheetTitle>Tools</SheetTitle>
          <SheetDescription>The MCP servers and tools this chat and its subagents can use. "Off here" and a tool's switch apply to this chat only.</SheetDescription>
        </SheetHeader>
        <div className="mx-4 mb-4 min-h-0 flex-1 overflow-y-auto">
          <Tools session={session} />
        </div>
      </SheetContent>
    </Sheet>
  )
}
