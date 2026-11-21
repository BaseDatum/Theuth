import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Loader2, Plug, Settings2 } from 'lucide-react'
import { useState } from 'react'
import { errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useDrafts } from '@/lib/drafts'
import { serverOps, serverState, useMcpHide, withDraftHides } from '@/lib/mcp-hide'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport, useSupported } from '@/rpc/capabilities'
import type { McpServer } from '@/rpc/bindings/method.mcp_2dserver.output'
import { BuiltinsMenu } from './BuiltinsMenu'
import { MenuCheck } from './MenuCheck'
import { mcpPowerRequirements, toolsRequirements } from './requirements'
import { useToolsSheet } from './tools-sheet'

/** Longest error text a row shows; the whole text is its tooltip. */
const ERROR_CHARS = 120

/** A switch flip in flight (`on`: where it goes) or the last one that failed (`error`). */
type Flip = { on: boolean; error?: string }

/** #4, 206: the composer's tools switch beside `+` — for this chat (`''`: the new-chat draft), one checkbox per
 * builtin category ("Builtins") and per MCP server ("MCP servers", each with a harness-wide on/off switch too), and
 * the chat's full Tools… sheet (per-tool switches) at the bottom, opened on the tab shown here. Hides go through
 * `lib/mcp-hide`, like the sheet's. An engine
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

/** The "MCP servers" panel. Per server row, two controls with different scopes:
 * - the checkbox shows or hides the server's tools in THIS chat (`''`: the new-chat draft) through `lib/mcp-hide`
 *   (`set_mcp_hidden` with `mcp:<server>`), like the chat's Tools… sheet;
 * - the switch turns the server on or off for the WHOLE harness, persistently: off is the `set_mcp_gate(server, null,
 *   'off')` the Tools page and the TUI's /mcp use (it stops the process and unregisters its tools everywhere); on
 *   clears an `off` gate (any other risk line stays) and waits for the process to connect (`mcp_start`). An engine
 *   without `mcp_start` keeps the off direction only; one without `set_mcp_gate` gets no switch. */
function McpServersMenu({ session, enabled }: { session: string; enabled: boolean }) {
  const api = useApi()
  const qc = useQueryClient()
  const canStart = useSupported(mcpPowerRequirements)
  const canStop = useQuerySupport(['set_mcp_gate'])
  const draftHidden = useDrafts((s) => s.settings.hidden)
  const servers = useQuery({
    enabled,
    queryKey: queryKeys.mcp(session),
    queryFn: () => api.list_mcp(session || null),
  })
  const keys = [queryKeys.mcp(session), queryKeys.mcp(null)]
  const hide = useMcpHide(session)
  const toggle = useAction((v: { server: McpServer; hidden: boolean }) => hide(serverOps(v.server, v.hidden)), { keys })
  const [flips, setFlips] = useState<Record<string, Flip>>({})

  const power = async (s: McpServer, on: boolean) => {
    setFlips((f) => ({ ...f, [s.name]: { on } }))
    // Only an `off` gate is cleared: a server-wide risk line (read_only, …) already means on.
    const wasOff = s.gate === 'off'
    let error: string | undefined
    try {
      if (!on) await api.set_mcp_gate(s.name, null, 'off')
      else {
        if (wasOff) await api.set_mcp_gate(s.name, null, null)
        try {
          await api.mcp_start(s.name)
        } catch (e) {
          // It did not start: put the harness-wide switch back where it was.
          if (wasOff) await api.set_mcp_gate(s.name, null, 'off').catch(() => {})
          throw e
        }
      }
    } catch (e) {
      error = errorText(e)
    }
    await Promise.all(keys.map((queryKey) => qc.invalidateQueries({ queryKey })))
    setFlips(({ [s.name]: _, ...rest }) => (error === undefined ? rest : { ...rest, [s.name]: { on, error } }))
  }

  const rows = (servers.data ?? []).map((s) => (session === '' ? withDraftHides(s, draftHidden) : s))
  return (
    <div role="menu" aria-label="MCP servers for this chat" className="max-h-72 overflow-y-auto">
      {servers.isPending && <p className="px-3 py-2 text-sm text-fg-tertiary">Loading…</p>}
      {servers.error && <p className="px-3 py-2 text-sm break-words text-destructive">{servers.error.message}</p>}
      {servers.data?.length === 0 && <p className="px-3 py-2 text-sm text-fg-tertiary">No MCP servers yet.</p>}
      {rows.map((s) => {
        const flip = flips[s.name]
        const busy = flip !== undefined && flip.error === undefined
        // Mid-flip the switch shows where it goes; a failed flip shows where it was. On = no `off` gate, and running
        // or connecting.
        const on = busy ? flip.on : s.gate !== 'off' && (s.connected || s.state === 'connecting')
        const pending = busy ? (flip.on ? 'starting…' : 'stopping…') : s.gate !== 'off' && s.state === 'connecting' ? 'starting…' : null
        // Something to show in this chat: running, on for the harness, with tools registered.
        const live = !busy && s.connected && s.gate !== 'off' && s.tools.some((t) => t.registered !== null)
        const state = serverState(s)
        const note = pending || flip?.error ? null : s.gate === 'off' ? 'off for every chat' : !s.connected ? (s.reason ?? 'not running') : null
        return (
          <div key={s.name}>
            <div className="flex items-center gap-1 pr-2">
              <MenuCheck
                label={s.name}
                state={state}
                live={live}
                busy={toggle.isPending}
                title={live ? 'Show or hide its tools in this chat' : busy ? undefined : s.gate === 'off' ? 'Off for every chat' : 'Not running'}
                count={live ? `${s.tools.length}` : ''}
                detail={
                  pending ? (
                    <span className="flex items-center gap-1 text-xs text-fg-tertiary">
                      <Loader2 className="size-3 animate-spin" aria-hidden="true" /> {pending}
                    </span>
                  ) : note ? (
                    <span className="block truncate text-xs text-fg-tertiary">{note}</span>
                  ) : null
                }
                onToggle={() => toggle.mutate({ server: s, hidden: state === 'on' })}
              />
              {canStop && (
                <Switch
                  size="sm"
                  aria-label={`${s.name} for every chat`}
                  title={canStart ? 'On or off for every chat (starts or stops the server)' : 'Off for every chat (stops the server)'}
                  checked={on}
                  disabled={busy || (!on && !canStart)}
                  onCheckedChange={(to) => void power(s, to)}
                />
              )}
            </div>
            {flip?.error && (
              <div className="flex items-start gap-2 pr-2 pb-1 pl-8 text-xs">
                <span role="alert" className="min-w-0 flex-1 break-words text-destructive" title={flip.error}>
                  {flip.error.length > ERROR_CHARS ? `${flip.error.slice(0, ERROR_CHARS)}…` : flip.error}
                </span>
                <button type="button" className="shrink-0 text-fg-accent hover:underline" onClick={() => void power(s, flip.on)}>
                  Retry
                </button>
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}
