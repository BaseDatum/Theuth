import { useQuery } from '@tanstack/react-query'
import { openUrl } from '@tauri-apps/plugin-opener'
import { ChevronRight, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { Column, Page } from '@/components/library/Page'
import { type Action, Dot, Empty, toastSync, useAction } from '@/components/settings/shared'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { gitUrl, mcpEntry } from '@/lib/mcpEntry'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Gate } from '@/rpc/bindings/Gate'
import type { McpServer } from '@/rpc/bindings/McpServer'
import type { McpToolInfo } from '@/rpc/bindings/McpToolInfo'
import type { Risk } from '@/rpc/bindings/Risk'
import type { SyncResult } from '@/rpc/bindings/SyncResult'

/** `tui/model.rs::Switch`: on, hidden in this session, or `off` in `mcp/config.yaml`. */
type SwitchState = 'on' | 'session' | 'always'

/** A server is `session` when every tool it has registered is hidden. */
function serverSwitch(s: McpServer): SwitchState {
  if (s.gate === 'off') return 'always'
  const live = s.tools.filter((t) => t.registered !== null)
  return live.length > 0 && live.every((t) => t.hidden) ? 'session' : 'on'
}

/** The gate select's `annotations` sentinel: no line in `mcp/config.yaml`. */
const ANNOTATIONS = 'annotations'
const RISKS: [Exclude<Gate, 'off'>, string][] = [
  ['read_only', 'Read-only'],
  ['mutating', 'Mutating'],
  ['destructive', 'Destructive'],
]
const RISK_NAME: Record<Risk, string> = {
  read_only: 'read-only',
  mutating: 'mutating',
  destructive: 'destructive',
}

/** The mutations every server and tool row shares; owned by `Tools`. */
type Actions = {
  gate: Action<{ server: string; tool: string | null; gate: Gate | null }, null>
  hidden: Action<{ names: string[]; hidden: boolean }, null>
  switchTo: Action<{ server: McpServer; to: SwitchState }, void>
  login: Action<string, void>
  logout: Action<string, null>
  remove: Action<string, null>
  sync: Action<void, SyncResult[]>
  add: Action<string, string>
}

function useMcpActions(session: string | null): Actions {
  const api = useApi()
  const keys = [queryKeys.mcp(session), queryKeys.mcp(null)]
  return {
    gate: useAction(
      (v: { server: string; tool: string | null; gate: Gate | null }) =>
        api.set_mcp_gate(v.server, v.tool, v.gate),
      { keys },
    ),
    hidden: useAction(
      (v: { names: string[]; hidden: boolean }) =>
        session
          ? api.set_mcp_hidden(session, v.names, v.hidden)
          : Promise.reject(new Error('Open a chat first')),
      { keys },
    ),
    // ⏎ in the TUI: turning on undoes whichever switch is off; off asks for the scope.
    switchTo: useAction(
      async (v: { server: McpServer; to: SwitchState }) => {
        const from = serverSwitch(v.server)
        const names = v.server.tools.flatMap((t) => (t.registered === null ? [] : [t.registered]))
        if (v.to === 'on') {
          if (from === 'session' && session) await api.set_mcp_hidden(session, names, false)
          else if (from === 'always') await api.set_mcp_gate(v.server.name, null, null)
        } else if (v.to === 'session') {
          if (!session) throw new Error('Open a chat first')
          await api.set_mcp_hidden(session, names, true)
        } else if (from !== 'always') await api.set_mcp_gate(v.server.name, null, 'off')
      },
      { keys },
    ),
    login: useAction((server: string) => api.mcp_login(server).then((url) => openUrl(url))),
    logout: useAction((server: string) => api.mcp_logout(server), { keys }),
    remove: useAction((server: string) => api.remove_mcp_server(server), {
      keys,
    }),
    sync: useAction((_: void) => api.sync_mcp(), {
      keys,
      onSuccess: toastSync,
    }),
    add: useAction(
      async (text: string) => {
        if (gitUrl(text)) return api.add_mcp_repo(text, null).then((r) => `Cloned ${r.name}`)
        const { name, entry } = mcpEntry(text)
        return api.add_mcp_server(name, entry).then((s) => `Added ${s.name}`)
      },
      { keys, onSuccess: (msg) => toast.success(msg) },
    ),
  }
}

/** `/tools`: the MCP servers (design doc 37 §4) — status, risk gates, login, per-tool overrides. `session`
 * (`?session=`) is the chat the page was opened from; the "off here" switches need one. */
export function Tools({ session }: { session: string | null }) {
  const api = useApi()
  const servers = useQuery({
    queryKey: queryKeys.mcp(session),
    queryFn: () => api.list_mcp(session),
  })
  const act = useMcpActions(session)
  return (
    <Page
      title="Tools"
      actions={
        <>
          <Button
            variant="ghost"
            size="sm"
            disabled={act.sync.isPending}
            onClick={() => act.sync.mutate(undefined)}
          >
            <RefreshCw className={act.sync.isPending ? 'animate-spin' : undefined} />
            Sync
          </Button>
          <AddPopover add={act.add} />
        </>
      }
    >
      <Column>
        {servers.data?.length === 0 && (
          <Empty>
            No tool servers yet. An MCP server gives the harness tools (files, browsers, APIs…); add one with a git URL
            or a JSON entry.
          </Empty>
        )}
        {servers.data?.map((s) => (
          <ServerRow key={s.name} server={s} session={session} act={act} />
        ))}
      </Column>
    </Page>
  )
}

function AddPopover({ add }: { add: Actions['add'] }) {
  const [open, setOpen] = useState(false)
  const [text, setText] = useState('')
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="sm">
          <Plus />
          Add
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[min(380px,calc(100vw-2rem))] space-y-2">
        <p className="text-sm text-fg-secondary">A git URL to clone, or one server as JSON.</p>
        <Textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={5}
          spellCheck={false}
          className="font-mono text-xs"
          placeholder={
            'https://github.com/org/server.git\n{"name": {"command": "npx", "args": ["-y", "…"]}}'
          }
        />
        <div className="flex justify-end">
          <Button
            size="sm"
            disabled={text.trim().length === 0 || add.isPending}
            onClick={() =>
              add.mutate(text.trim(), {
                onSuccess: () => {
                  setText('')
                  setOpen(false)
                },
              })
            }
          >
            Add
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  )
}

function ServerRow({
  server: s,
  session,
  act,
}: {
  server: McpServer
  session: string | null
  act: Actions
}) {
  const [open, setOpen] = useState(false)
  const sw = serverSwitch(s)
  const live = s.tools.some((t) => t.registered !== null)
  const on = sw !== 'always' && s.connected
  const status = sw === 'always' ? 'off' : s.connected ? 'connected' : s.auth === 'required' ? 'login required' : 'not connected'
  const sessionHint = !session ? 'Open a chat first' : !live ? 'Not connected' : undefined
  return (
    <Collapsible
      open={open}
      onOpenChange={setOpen}
      className="border-b border-border-subtle py-3 last:border-0"
    >
      <div className="flex items-center gap-3">
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="flex min-w-0 flex-1 items-center gap-2 text-left"
            disabled={s.tools.length === 0}
          >
            <ChevronRight
              className={`size-4 shrink-0 text-fg-tertiary transition-transform ${open ? 'rotate-90' : ''}`}
            />
            <span className="wrap-anywhere text-sm font-medium">{s.name}</span>
            <span className="shrink-0 text-[13px] text-fg-tertiary">
              {s.connected ? `${s.tools.length} tools` : '–'}
            </span>
            {s.repo && (
              <Badge variant="outline" className="shrink-0 font-normal">
                ⟳ {s.repo}
              </Badge>
            )}
          </button>
        </CollapsibleTrigger>
        <span className="flex shrink-0 items-center gap-1.5 text-[13px] text-fg-secondary">
          <Dot on={on} />
          {status}
        </span>
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label={`Remove ${s.name}`}>
              <Trash2 />
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Remove {s.name}?</AlertDialogTitle>
              <AlertDialogDescription>
                Its entry is deleted from the config file, or disabled when the file holds other
                servers
                {s.repo ? '; the clone stays on disk' : ''}.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={() => act.remove.mutate(s.name)}>
                Remove
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
      {/* Controls on their own line: three of them beside the name squashed it. */}
      <div className="mt-2 ml-6 flex flex-wrap items-center gap-2">
        <span title={sessionHint}>
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            value={sw}
            onValueChange={(v) => v && act.switchTo.mutate({ server: s, to: v as SwitchState })}
          >
            <ToggleGroupItem value="on">On</ToggleGroupItem>
            <ToggleGroupItem value="session" disabled={sessionHint !== undefined}>
              Off here
            </ToggleGroupItem>
            <ToggleGroupItem value="always">Off always</ToggleGroupItem>
          </ToggleGroup>
        </span>
        <GateSelect
          value={s.gate === 'off' ? null : s.gate}
          disabled={sw === 'always'}
          onChange={(gate) => act.gate.mutate({ server: s.name, tool: null, gate })}
        />
        {s.auth !== 'none' && (
          <Button
            variant="outline"
            size="sm"
            onClick={() =>
              s.auth === 'required' ? act.login.mutate(s.name) : act.logout.mutate(s.name)
            }
          >
            {s.auth === 'required' ? 'Login' : 'Logout'}
          </Button>
        )}
      </div>
      <CollapsibleContent className="mt-2 ml-6">
        {s.tools.map((t) => (
          <ToolRow key={t.name} server={s.name} tool={t} session={session} act={act} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  )
}

function ToolRow({
  server,
  tool: t,
  session,
  act,
}: {
  server: string
  tool: McpToolInfo
  session: string | null
  act: Actions
}) {
  // `mutating* (ask)`: effective risk, `*` when the tool's own line sets it, then what `tools.policy` does with it.
  const risk = t.gate === 'off' ? '' : `${RISK_NAME[t.risk]}${t.gate ? '*' : ''} (${t.action})`
  return (
    <div className="flex items-start gap-3 border-b border-border-subtle py-2 last:border-0">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2 text-sm">
          <span className="wrap-anywhere">{t.name}</span>
          {t.registered && t.registered !== t.name && (
            <span className="text-[13px] text-fg-tertiary">as {t.registered}</span>
          )}
        </div>
        <div className="text-[13px] wrap-break-word text-fg-secondary">{t.description}</div>
      </div>
      <div className="flex shrink-0 items-center gap-3">
        <span className="text-[13px] text-fg-tertiary">{risk}</span>
        <GateSelect
          value={t.gate}
          off
          onChange={(gate) => act.gate.mutate({ server, tool: t.name, gate })}
        />
        <Switch
          checked={!t.hidden}
          disabled={!session || t.registered === null}
          title={!session ? 'Open a chat first' : undefined}
          onCheckedChange={(v) =>
            t.registered && act.hidden.mutate({ names: [t.registered], hidden: !v })
          }
        />
      </div>
    </div>
  )
}

/** Risk override: `annotations` (no line) or a `Gate`; `off` only where a tool may be switched off for good. */
function GateSelect({
  value,
  off,
  disabled,
  onChange,
}: {
  value: Gate | null
  off?: boolean
  disabled?: boolean
  onChange: (gate: Gate | null) => void
}) {
  return (
    <Select
      value={value ?? ANNOTATIONS}
      disabled={disabled}
      onValueChange={(v) => onChange(v === ANNOTATIONS ? null : (v as Gate))}
    >
      <SelectTrigger size="sm" className="w-32">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={ANNOTATIONS}>Annotations</SelectItem>
        {RISKS.map(([id, label]) => (
          <SelectItem key={id} value={id}>
            {label}
          </SelectItem>
        ))}
        {off && <SelectItem value="off">Off always</SelectItem>}
      </SelectContent>
    </Select>
  )
}
