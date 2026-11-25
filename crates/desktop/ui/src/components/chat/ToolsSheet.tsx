import { useRouterState } from '@tanstack/react-router'
import { useEffect } from 'react'
import { Builtins } from '@/components/library/Builtins'
import { ToolCategories } from '@/components/library/ToolCategories'
import { Tools } from '@/components/library/Tools'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useQuerySupport } from '@/rpc/capabilities'
import { type ToolsTab, useToolsSheet } from './tools-sheet'

/** One chat's tools (Settings › Tools › Builtins and MCP scoped to it): "Off here" and the per-tool switches hide them
 * from this chat and its subagents. Opened from its ⋯ menu (Tools…), `/tools`, `/mcp` or the composer's tools popover,
 * on the tab that popover showed. An engine without `list_builtins` (206) gets the MCP list alone. */
export function ToolsSheet({ session }: { session: string }) {
  const open = useToolsSheet((s) => s.session === session)
  const chosen = useToolsSheet((s) => s.tab)
  const toolCategories = useQuerySupport(['list_tool_categories', 'set_mcp_hidden'])
  const builtins = useQuerySupport(['list_builtins'])
  const tab: ToolsTab =
    (chosen === 'categories' && toolCategories) || (chosen === 'builtins' && builtins) || chosen === 'mcp'
      ? chosen
      : toolCategories
        ? 'categories'
        : builtins
          ? 'builtins'
          : 'mcp'
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
          <SheetDescription>
            {toolCategories
              ? 'The tool categories, built-in tools and MCP servers this chat and its subagents can use. Category and "Off here" switches apply to this chat; defaults apply to new chats.'
              : builtins
                ? 'The built-in tools and MCP servers this chat and its subagents can use. "Off here" and a tool\'s switch apply to this chat only; "Off always" to every chat.'
                : 'The MCP servers and tools this chat and its subagents can use. "Off here" and a tool\'s switch apply to this chat only.'}
          </SheetDescription>
        </SheetHeader>
        <div className="mx-4 mb-4 min-h-0 flex-1 overflow-y-auto">
          {toolCategories || builtins ? (
            <Tabs value={tab} onValueChange={(v) => useToolsSheet.getState().setTab(v as ToolsTab)}>
              <TabsList className="w-full">
                {toolCategories && <TabsTrigger value="categories">Categories</TabsTrigger>}
                {builtins && <TabsTrigger value="builtins">Builtins</TabsTrigger>}
                <TabsTrigger value="mcp">MCP servers</TabsTrigger>
              </TabsList>
              {toolCategories && (
                <TabsContent value="categories" className="pt-2">
                  <ToolCategories session={session} />
                </TabsContent>
              )}
              {builtins && (
                <TabsContent value="builtins" className="pt-2">
                  <Builtins session={session} />
                </TabsContent>
              )}
              <TabsContent value="mcp" className="pt-2">
                <Tools session={session} />
              </TabsContent>
            </Tabs>
          ) : (
            <Tools session={session} />
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}
