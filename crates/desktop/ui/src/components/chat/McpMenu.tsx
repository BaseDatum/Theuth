import { useQuery } from '@tanstack/react-query'
import { Plug, Settings2 } from 'lucide-react'
import { useState } from 'react'
import { useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useDrafts } from '@/lib/drafts'
import { serverOps, serverState, useMcpHide, withDraftHides } from '@/lib/mcp-hide'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport, useSupported } from '@/rpc/capabilities'
import type { McpServer } from '@/rpc/bindings/method.mcp_2dserver.output'
import { BuiltinsMenu } from './BuiltinsMenu'
import { MenuCheck } from './MenuCheck'
import { toolsRequirements } from './requirements'
import { useToolsSheet } from './tools-sheet'

/** #4, 206: the composer's tools switch beside `+` — for this chat (`''`: the new-chat draft), one checkbox per
 * builtin category ("Builtins") and per MCP server ("MCP servers"), and the chat's full Tools… sheet (per-tool
 * switches) at the bottom, opened on the tab shown here. Hides go through `lib/mcp-hide`, like the sheet's. An engine
 * without `list_builtins` gets the server list alone (today's popover), one without the MCP methods the Builtins panel
 * alone; the bottom button only where the sheet is mounted (`Header` gates it on `toolsRequirements`). */
export function McpMenu({ session, className }: { session: string; className?: string }) {
  const mcp = useQuerySupport(['list_mcp', 'set_mcp_hidden'])
  const builtins = useQuerySupport(['list_builtins', 'set_mcp_hidden'])
  const sheet = useSupported(toolsRequirements)
  const [open, setOpen] = useState(false)
  const chosen = useToolsSheet((s) => s.tab)
  const setTab = useToolsSheet((s) => s.setTab)
  if (!mcp && !builtins) return null
  const tab = !builtins ? 'mcp' : !mcp ? 'builtins' : chosen
  const servers = <McpServersMenu session={session} enabled={mcp && open && tab === 'mcp'} />
  const categories = <BuiltinsMenu session={session} enabled={builtins && open && tab === 'builtins'} />
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          className={cn('size-9 shrink-0 rounded-full text-fg-secondary', className)}
          aria-label={builtins ? 'Tools and MCP servers' : 'MCP servers'}
        >
          <Plug className="size-5" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className={cn('p-1', builtins ? 'w-72' : 'w-64')}>
        {mcp && builtins ? (
          <Tabs value={tab} onValueChange={(v) => setTab(v === 'mcp' ? 'mcp' : 'builtins')} className="gap-1">
            <TabsList className="w-full">
              <TabsTrigger value="builtins">Builtins</TabsTrigger>
              <TabsTrigger value="mcp">MCP servers</TabsTrigger>
            </TabsList>
            <TabsContent value="builtins">{categories}</TabsContent>
            <TabsContent value="mcp">{servers}</TabsContent>
          </Tabs>
        ) : mcp ? (
          servers
        ) : (
          categories
        )}
        {sheet && (
          <div className="mt-1 border-t border-border-subtle pt-1">
            <Button
              variant="ghost"
              className="w-full justify-start"
              onClick={() => {
                setOpen(false)
                useToolsSheet.getState().show(session, tab)
              }}
            >
              <Settings2 /> All tools & servers…
            </Button>
          </div>
        )}
      </PopoverContent>
    </Popover>
  )
}

/** The "MCP servers" panel: one checkbox per server for this chat. */
function McpServersMenu({ session, enabled }: { session: string; enabled: boolean }) {
  const api = useApi()
  const draftHidden = useDrafts((s) => s.settings.hidden)
  const servers = useQuery({
    enabled,
    queryKey: queryKeys.mcp(session),
    queryFn: () => api.list_mcp(session || null),
  })
  const hide = useMcpHide(session)
  const toggle = useAction((v: { server: McpServer; hidden: boolean }) => hide(serverOps(v.server, v.hidden)), {
    keys: [queryKeys.mcp(session), queryKeys.mcp(null)],
  })
  const rows = (servers.data ?? []).map((s) => (session === '' ? withDraftHides(s, draftHidden) : s))
  return (
    <div role="menu" aria-label="MCP servers for this chat" className="max-h-72 overflow-y-auto">
      {servers.isPending && <p className="px-3 py-2 text-sm text-fg-tertiary">Loading…</p>}
      {servers.error && <p className="px-3 py-2 text-sm break-words text-destructive">{servers.error.message}</p>}
      {servers.data?.length === 0 && <p className="px-3 py-2 text-sm text-fg-tertiary">No MCP servers yet.</p>}
      {rows.map((s) => {
        const live = s.connected && s.gate !== 'off' && s.tools.some((t) => t.registered !== null)
        const state = serverState(s)
        return (
          <MenuCheck
            key={s.name}
            label={s.name}
            state={state}
            live={live}
            busy={toggle.isPending}
            title={live ? undefined : s.gate === 'off' ? 'Off always (Settings › Tools › MCP)' : 'Not connected'}
            count={live ? `${s.tools.length}` : s.gate === 'off' ? 'off' : '–'}
            onToggle={() => toggle.mutate({ server: s, hidden: state === 'on' })}
          />
        )
      })}
    </div>
  )
}
