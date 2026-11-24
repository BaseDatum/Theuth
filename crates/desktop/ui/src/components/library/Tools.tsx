import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { useSearch } from '@tanstack/react-router'
import { ChevronRight, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { Fragment, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { type HomeAction, homeActions, homeChip, stateText } from '@/components/library/mcp-home'
import { segments } from '@/components/library/names'
import { type Action, Dot, Empty, Section, errorText, toastSync, useAction } from '@/components/settings/shared'
import { type Engine, engines } from '@/components/tasks/home'
import { useMesh } from '@/components/tasks/queries'
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
import { useDrafts } from '@/lib/drafts'
import { type HideOp, serverOps, toolOps, useMcpHide, withDraftHides } from '@/lib/mcp-hide'
import { queryKeys, useApi } from '@/lib/rpc'
import { LOCAL, useEngines } from '@/link/engines'
import type { Gate } from "@/rpc/bindings/method.mcp_2dserver.output"
import type { McpHomeAction } from '@/rpc/bindings/method.mcp_2dserver.input'
import type { McpServer } from "@/rpc/bindings/method.mcp_2dserver.output"
import type { McpToolInfo } from "@/rpc/bindings/method.mcp_2dserver.output"
import type { Risk } from "@/rpc/bindings/method.agent.output"
import type { SyncResult } from "@/rpc/bindings/method.agent.output"
import { useConnection } from '@/store/connection'

/** A switch is on, hidden in this chat, or globally off. */
type SwitchState = 'on' | 'session' | 'always'

/** A server is `session` when every tool it has registered is hidden. */
function serverSwitch(s: McpServer): SwitchState {
  if (s.gate === 'off') return 'always'
  const live = s.tools.filter((t) => t.registered !== null)
  return live.length > 0 && live.every((t) => t.hidden) ? 'session' : 'on'
}

/** The gate select's `annotations` sentinel: no explicit risk override. */
const ANNOTATIONS = 'annotations'
/** Why a "this chat" switch is greyed on Settings › Tools › MCP: it needs the chat it would apply to. */
export const CHAT_HINT = 'Open from a chat: ⋯ › Tools… or /tools'
const RISKS: [Exclude<Gate, 'off'>, string][] = [
  ['read_only', 'Read-only'],
  ['mutating', 'Mutating'],
  ['destructive', 'Destructive'],
]
export const RISK_NAME: Record<Risk, string> = {
  read_only: 'read-only',
  mutating: 'mutating',
  destructive: 'destructive',
}

/** The mutations every server and tool row shares; owned by `Tools`. */
type Actions = {
  gate: Action<{ server: string; tool: string | null; gate: Gate | null }, null>
  hidden: Action<HideOp[], null>
  switchTo: Action<{ server: McpServer; to: SwitchState }, void>
  /** The sign-in URL the engine opened on its host, and whether that host is this machine (`local`). */
  login: Action<string, { url: string; local: boolean }>
  logout: Action<string, null>
  home: Action<{ server: string; action: McpHomeAction }, null>
  remove: Action<string, null>
  sync: Action<void, SyncResult[]>
  add: Action<string, string>
}

function useMcpActions(session: string | null): Actions {
  const api = useApi()
  const active = useEngines((s) => s.active)
  const keys = [queryKeys.mcp(session), queryKeys.mcp(null)]
  const hide = useMcpHide(session)
  return {
    gate: useAction(
      (v: { server: string; tool: string | null; gate: Gate | null }) =>
        api.set_mcp_gate(v.server, v.tool, v.gate),
      { keys },
    ),
    hidden: useAction(hide, { keys }),
    // ⏎ in the TUI: turning on undoes whichever switch is off; off asks for the scope.
    switchTo: useAction(
      async (v: { server: McpServer; to: SwitchState }) => {
        const from = serverSwitch(v.server)
        if (v.to === 'on') {
          if (from === 'session') await hide(serverOps(v.server, false))
          else if (from === 'always') await api.set_mcp_gate(v.server.name, null, null)
        } else if (v.to === 'session') {
          await hide(serverOps(v.server, true))
        } else if (from !== 'always') await api.set_mcp_gate(v.server.name, null, 'off')
      },
      { keys },
    ),
    // As Settings' provider logins: the engine's `mcp_login` opens the browser on its own host (here, or the node over
    // link, 31 §9), so the URL is only shown here; opening it here too would start a second sign-in.
    login: useAction(async (server: string) => ({ url: await api.mcp_login(server), local: active === LOCAL })),
    logout: useAction((server: string) => api.mcp_logout(server), { keys }),
    // 187: which machine runs it (`mcp/homes.yaml`).
    home: useAction((v: { server: string; action: McpHomeAction }) => api.set_mcp_home(v.server, v.action), { keys }),
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

/** Settings › Tools › MCP: the MCP servers (design doc 37 §4) — status, risk gates, login, home machine (187), per-tool
 * overrides. `?session=` is the chat an old `/tools?session=` link carried; the "off here" switches need one. In a chat,
 * ⋯ › Tools…, `/tools` and `/mcp` open these switches in the chat's own sheet (`ToolsSheet`). */
export function McpServersTab() {
  const session = useSearch({ from: '/settings/$tab', select: (s) => s.session ?? null })
  return <Tools session={session} />
}

export function Tools({ session }: { session: string | null }) {
  const canQuery1 = useQuerySupport(["list_mcp"])
  const canQuery1_2 = useQuerySupport(["mcp_status"])
  const api = useApi()
  const hidden = useDrafts((s) => s.settings.hidden)
  const servers = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.mcp(session),
    queryFn: () => api.list_mcp(session || null),
  })
  // Under the `mcp` prefix: every `McpProgress` event refreshes it with the list.
  const status = useQuery({ enabled: canQuery1_2, queryKey: ['mcp', 'status'], queryFn: () => api.mcp_status() })
  const mesh = useMesh()
  // Until the mesh answers, no home chip and no home actions: "on this machine" would flicker to the real name.
  const machines = mesh.isPending ? null : engines(mesh.data)
  const act = useMcpActions(session)
  return (
    <Section
      title="Servers"
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
      {status.data?.homes_problem && (
        <p className="py-2 text-[13px] break-words text-destructive">
          {status.data.homes_problem} — homes stay as they were and nothing is written until it reads again.
        </p>
      )}
      {servers.error && <p className="py-2 text-[13px] break-words text-destructive">{errorText(servers.error)}</p>}
      {servers.data?.length === 0 && (
        <Empty>
          No tool servers yet. An MCP server gives the harness tools (files, browsers, APIs…); add one with a git URL
          or a JSON entry.
        </Empty>
      )}
      {servers.data?.map((server) => {
        const s = session === '' ? withDraftHides(server, hidden) : server
        return <ServerRow key={s.name} server={s} session={session} machines={machines} act={act} />
      })}
    </Section>
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
  machines,
  act,
}: {
  server: McpServer
  session: string | null
  /** The mesh's engines; null until it answers. */
  machines: Engine[] | null
  act: Actions
}) {
  const [open, setOpen] = useState(false)
  const [pending, setPending] = useState<{ url: string; local: boolean } | null>(null)
  const engineName = useConnection((st) => st.info?.name)
  const sw = serverSwitch(s)
  const live = s.tools.some((t) => t.registered !== null)
  const on = sw !== 'always' && s.connected
  // 187: a server not running here says why (homed elsewhere, not installed here, …).
  const status = sw === 'always' ? 'off' : s.connected ? 'connected' : s.auth === 'required' ? 'login required' : stateText(s, machines ?? [])
  const sessionHint = session === null ? CHAT_HINT : !live ? 'Not connected' : undefined
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
            className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1 text-left sm:flex-nowrap"
            disabled={s.tools.length === 0}
          >
            <ChevronRight
              className={`size-4 shrink-0 text-fg-tertiary transition-transform ${open ? 'rotate-90' : ''}`}
            />
            <span className="text-sm font-medium break-words">
              <Name name={s.name} />
            </span>
            <span className="shrink-0 text-[13px] text-fg-tertiary">
              {s.connected ? `${s.tools.length} tools` : '–'}
            </span>
            {s.repo && (
              <Badge variant="outline" className="shrink-0 font-normal">
                ⟳ {s.repo}
              </Badge>
            )}
            {machines && (
              <Badge variant="outline" className="shrink-0 font-normal">
                {homeChip(s, machines)}
              </Badge>
            )}
          </button>
        </CollapsibleTrigger>
        <Status on={on} label={status} className="hidden sm:flex" />
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
                Its entry is deleted, or disabled when it is defined alongside other servers
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
      {/* Controls on their own line: three of them beside the name squashed it. On a phone the status joins them. */}
      <div className="mt-2 ml-6 flex flex-wrap items-center gap-2">
        <Status on={on} label={status} className="sm:hidden" />
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
          <>
            {s.auth === 'logged_in' && <span className="text-[13px] text-fg-secondary">logged in</span>}
            <Button
              variant="outline"
              size="sm"
              disabled={act.login.isPending || act.logout.isPending}
              onClick={() =>
                s.auth === 'required' ? act.login.mutate(s.name, { onSuccess: setPending }) : act.logout.mutate(s.name)
              }
            >
              {s.auth === 'required' ? 'Log in' : 'Log out'}
            </Button>
          </>
        )}
        {machines && homeActions(s, machines).map((a) => <HomeButton key={a.label} server={s.name} action={a} act={act} />)}
      </div>
      {s.reason && <p className="mt-1 ml-6 text-[13px] break-words text-fg-tertiary">{s.reason}</p>}
      {pending && s.auth === 'required' && (
        <p className="mt-1 ml-6 text-[13px] text-fg-secondary">
          {pending.local ? "Browser didn't open?" : `Sign in from the node (${engineName ?? 'node'}):`}{' '}
          <code className="select-all break-all font-mono text-xs">{pending.url}</code>
        </p>
      )}
      <CollapsibleContent className="mt-2 ml-6">
        {s.tools.map((t) => (
          <ToolRow key={t.name} server={s} tool={t} session={session} act={act} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  )
}

/** 187: one of a row's home actions (Run everywhere, Only on <home>, Move home here, Turn off / on here). */
function HomeButton({ server, action: a, act }: { server: string; action: HomeAction; act: Actions }) {
  return (
    <Button
      size="sm"
      variant={a.pressed ? 'secondary' : 'outline'}
      aria-pressed={a.pressed}
      className="pointer-coarse:min-h-11"
      disabled={act.home.isPending}
      onClick={() => act.home.mutate({ server, action: a.action })}
    >
      {a.label}
    </Button>
  )
}

function ToolRow({
  server,
  tool: t,
  session,
  act,
}: {
  server: McpServer
  tool: McpToolInfo
  session: string | null
  act: Actions
}) {
  // `mutating* (ask)`: effective risk, `*` when the tool's own line sets it, then what `tools.policy` does with it.
  const risk = t.gate === 'off' ? '' : `${RISK_NAME[t.risk]}${t.gate ? '*' : ''} (${t.action})`
  return (
    // 113: a phone stacks the controls under the name; beside it they left the name a few px wide.
    <div className="flex flex-col gap-2 border-b border-border-subtle py-2 last:border-0 sm:flex-row sm:items-start sm:gap-3">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2 text-sm break-words">
          <span>
            <Name name={t.name} />
          </span>
          {t.registered && t.registered !== t.name && (
            <span className="text-[13px] text-fg-tertiary">
              as <Name name={t.registered} />
            </span>
          )}
        </div>
        <div className="text-[13px] wrap-break-word text-fg-secondary">{t.description}</div>
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-3">
        <span className="text-[13px] text-fg-tertiary">{risk}</span>
        <GateSelect
          value={t.gate}
          off
          onChange={(gate) => act.gate.mutate({ server: server.name, tool: t.name, gate })}
        />
        <Switch
          checked={!t.hidden}
          disabled={session === null || t.registered === null}
          title={session === null ? CHAT_HINT : undefined}
          onCheckedChange={(v) => t.registered && act.hidden.mutate(toolOps(server, t.registered, !v))}
        />
      </div>
    </div>
  )
}

/** A server or tool name with a `<wbr/>` after every underscore run: wraps at `_`/`__`, never mid-word. */
export function Name({ name }: { name: string }) {
  return segments(name).map((seg, i) => (
    <Fragment key={i}>
      {i > 0 && <wbr />}
      {seg}
    </Fragment>
  ))
}

/** The connection dot and its word; twice in `ServerRow`: beside the name above `sm`, first of the controls below it. */
function Status({ on, label, className }: { on: boolean; label: string; className: string }) {
  return (
    <span className={`${className} shrink-0 items-center gap-1.5 text-[13px] text-fg-secondary`}>
      <Dot on={on} />
      {label}
    </span>
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
