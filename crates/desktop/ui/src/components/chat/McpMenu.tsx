import { useQuery } from '@tanstack/react-query'
import { Check, Minus, Plug, Settings2 } from 'lucide-react'
import { useState } from 'react'
import { useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useDrafts } from '@/lib/drafts'
import { serverOps, serverState, useMcpHide, withDraftHides } from '@/lib/mcp-hide'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import type { McpServer } from '@/rpc/bindings/method.mcp_2dserver.output'
import { useToolsSheet } from './tools-sheet'

/** #4: the composer's MCP switch beside `+` — one checkbox per server for this chat (`''`: the new-chat draft), and
 * the chat's full Tools… sheet (per-tool switches) at the bottom. Hides go through `lib/mcp-hide`, like the sheet's. */
export function McpMenu({ session, className }: { session: string; className?: string }) {
  const supported = useQuerySupport(['list_mcp', 'set_mcp_hidden'])
  const [open, setOpen] = useState(false)
  const api = useApi()
  const draftHidden = useDrafts((s) => s.settings.hidden)
  const servers = useQuery({
    enabled: supported && open,
    queryKey: queryKeys.mcp(session),
    queryFn: () => api.list_mcp(session || null),
  })
  const hide = useMcpHide(session)
  const toggle = useAction((v: { server: McpServer; hidden: boolean }) => hide(serverOps(v.server, v.hidden)), {
    keys: [queryKeys.mcp(session), queryKeys.mcp(null)],
  })
  if (!supported) return null
  const rows = (servers.data ?? []).map((s) => (session === '' ? withDraftHides(s, draftHidden) : s))
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" className={cn('size-9 shrink-0 rounded-full text-fg-secondary', className)} aria-label="MCP servers">
          <Plug className="size-5" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 p-1">
        <div role="menu" aria-label="MCP servers for this chat" className="max-h-72 overflow-y-auto">
          {servers.isPending && <p className="px-3 py-2 text-sm text-fg-tertiary">Loading…</p>}
          {servers.error && <p className="px-3 py-2 text-sm break-words text-destructive">{servers.error.message}</p>}
          {servers.data?.length === 0 && <p className="px-3 py-2 text-sm text-fg-tertiary">No MCP servers yet.</p>}
          {rows.map((s) => {
            const live = s.connected && s.gate !== 'off' && s.tools.some((t) => t.registered !== null)
            const state = serverState(s)
            return (
              <button
                key={s.name}
                type="button"
                role="menuitemcheckbox"
                aria-checked={state === 'mixed' ? 'mixed' : state === 'on'}
                disabled={!live || toggle.isPending}
                title={live ? undefined : s.gate === 'off' ? 'Off always (Settings › Tools › MCP)' : 'Not connected'}
                onClick={() => toggle.mutate({ server: s, hidden: state === 'on' })}
                className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent disabled:pointer-events-none disabled:opacity-50"
              >
                <span
                  aria-hidden="true"
                  className={cn(
                    'flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-border',
                    live && state !== 'off' && 'border-fg-accent bg-fg-accent text-white',
                  )}
                >
                  {live && state === 'on' && <Check className="size-3" />}
                  {live && state === 'mixed' && <Minus className="size-3" />}
                </span>
                <span className="min-w-0 flex-1 truncate">{s.name}</span>
                <span className="shrink-0 text-xs text-fg-tertiary">{live ? `${s.tools.length}` : s.gate === 'off' ? 'off' : '–'}</span>
              </button>
            )
          })}
        </div>
        <div className="mt-1 border-t border-border-subtle pt-1">
          <Button
            variant="ghost"
            className="w-full justify-start"
            onClick={() => {
              setOpen(false)
              useToolsSheet.getState().show(session)
            }}
          >
            <Settings2 /> All tools & servers…
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  )
}
