import { useQuery } from '@tanstack/react-query'
import { useSearch } from '@tanstack/react-router'
import { ChevronRight, Search } from 'lucide-react'
import { useState } from 'react'
import { CHAT_HINT, Name, RISK_NAME } from '@/components/library/Tools'
import { AlwaysAskSelect } from '@/components/library/AlwaysAskSelect'
import { type Action, Empty, Section, errorText, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import {
  type BuiltinCategory,
  type BuiltinOps,
  type BuiltinSwitch,
  type BuiltinTool,
  categoryOps,
  categorySwitch,
  toolOps,
  toolSwitch,
  useBuiltinsApply,
  withDraftHides,
} from '@/lib/builtins'
import { useDrafts } from '@/lib/drafts'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { alwaysAskSummary } from '@/lib/always-ask'
import { useConnection } from '@/store/connection'

/** Settings › Tools › Builtins (206): the harness's own tools by category, the MCP tab's twin. `?session=` scopes the
 * "Off here" switches to that chat; in a chat, the Tools sheet shows the same list for it. */
export function BuiltinsTab() {
  const session = useSearch({ from: '/settings/$tab', select: (s) => s.session ?? null })
  return <Builtins session={session} />
}

/** One category as the search shows it: the whole category (its ops need every tool) and the tools that match. */
type Shown = { category: BuiltinCategory; tools: BuiltinTool[] }

/** A category whose label, description or id matches shows every tool; otherwise only tools whose name or
 * description matches. */
function search(cats: BuiltinCategory[], query: string): Shown[] {
  const q = query.trim().toLowerCase()
  if (!q) return cats.map((category) => ({ category, tools: category.tools }))
  return cats.flatMap((category) => {
    if ([category.label, category.description, category.id].some((s) => s.toLowerCase().includes(q))) return [{ category, tools: category.tools }]
    const tools = category.tools.filter((t) => t.name.toLowerCase().includes(q) || t.description.toLowerCase().includes(q))
    return tools.length ? [{ category, tools }] : []
  })
}

/** `session`: a chat id, `''` for the not-yet-created chat (its hides live in the draft), or null (Settings, no chat:
 * only the harness-wide switch). */
export function Builtins({ session }: { session: string | null }) {
  const canQuery = useQuerySupport(['list_builtins'])
  const api = useApi()
  const draft = useDrafts((s) => s.settings.hidden)
  const list = useQuery({
    enabled: canQuery,
    queryKey: queryKeys.builtins(session),
    queryFn: () => api.list_builtins(session || null),
  })
  // No `keys`: `useBuiltinsApply` itself awaits the refetch of every builtins list, success or failure.
  const apply = useAction(useBuiltinsApply(session))
  const [query, setQuery] = useState('')
  const cats = session === '' ? withDraftHides(list.data ?? [], draft) : (list.data ?? [])
  const shown = search(cats, query)
  const searching = query.trim()
  const approvalsOff = useConnection((s) => s.info?.approvals.global === false)
  return (
    <Section title="Built-in tools">
      {session === null && <p className="text-[13px] text-fg-tertiary">"Off here" needs a chat. {CHAT_HINT}</p>}
      {session === null && approvalsOff && (
        <p data-approvals-off="" className="text-[13px] text-fg-tertiary">
          Approvals are off: built-ins run without asking, except what always asks (each row says so).
        </p>
      )}
      <div className="relative py-2">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-fg-tertiary" />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === 'Escape' && setQuery('')}
          placeholder="Search built-in tools"
          aria-label="Search built-in tools"
          className="h-9 rounded-[10px] bg-layer-2 pl-8"
          spellCheck={false}
        />
      </div>
      {canQuery && list.isPending && <p className="py-2 text-[13px] text-fg-tertiary">Loading…</p>}
      {list.error && <p className="py-2 text-[13px] break-words text-destructive">{errorText(list.error)}</p>}
      {list.data?.length === 0 && <Empty>This engine has no built-in tools to switch.</Empty>}
      {list.data && list.data.length > 0 && shown.length === 0 && <Empty>No built-in tool matches "{query.trim()}".</Empty>}
      {shown.map(({ category, tools }) => (
        <CategoryRow key={category.id} category={category} tools={tools} search={searching} session={session} apply={apply} />
      ))}
    </Section>
  )
}

function CategoryRow({
  category: c,
  tools,
  search: q,
  session,
  apply,
}: {
  category: BuiltinCategory
  /** The tools the search shows (all of them without a search). */
  tools: BuiltinTool[]
  /** The active search text: while it is set the row opens, until the user closes it for this very text. */
  search: string
  session: string | null
  apply: Action<BuiltinOps, null>
}) {
  const [open, setOpen] = useState(false)
  // A click during a search wins until the search text changes.
  const [during, setDuring] = useState<{ q: string; open: boolean } | null>(null)
  const shown = q ? (during?.q === q ? during.open : true) : open
  const sw = categorySwitch(c)
  const on = c.tools.filter((t) => toolSwitch(t) === 'on').length
  return (
    <Collapsible
      open={shown}
      onOpenChange={(o) => (q ? setDuring({ q, open: o }) : setOpen(o))}
      className="border-b border-border-subtle py-3 last:border-0"
    >
      <CollapsibleTrigger asChild>
        <button type="button" className="flex w-full min-w-0 items-start gap-2 text-left pointer-coarse:min-h-11">
          <ChevronRight
            className={`mt-0.5 size-4 shrink-0 text-fg-tertiary transition-transform ${shown ? 'rotate-90' : ''}`}
          />
          <span className="min-w-0 flex-1">
            <span className="flex flex-wrap items-baseline gap-x-2">
              <span className="text-sm font-medium break-words">{c.label}</span>
              <span className="shrink-0 text-[13px] text-fg-tertiary">
                {on} of {c.tools.length} on
              </span>
            </span>
            <span className="block text-[13px] wrap-break-word text-fg-secondary">{c.description}</span>
          </span>
        </button>
      </CollapsibleTrigger>
      <div className="mt-2 ml-6 flex flex-wrap items-center gap-2">
        <SwitchGroup
          value={sw}
          session={session}
          label={c.label}
          disabled={apply.isPending}
          onChange={(to) => apply.mutate(categoryOps(c, to, session !== null))}
        />
        {sw === 'mixed' && (
          <Badge variant="outline" className="font-normal">
            Mixed
          </Badge>
        )}
      </div>
      <CollapsibleContent className="mt-2 ml-6">
        {tools.map((t) => (
          <ToolRow key={t.name} category={c} tool={t} session={session} apply={apply} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  )
}

function ToolRow({
  category,
  tool: t,
  session,
  apply,
}: {
  category: BuiltinCategory
  tool: BuiltinTool
  session: string | null
  apply: Action<BuiltinOps, null>
}) {
  return (
    <div className="flex flex-col gap-2 border-b border-border-subtle py-2 last:border-0 sm:flex-row sm:items-start sm:gap-3">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2 text-sm break-words">
          <span className="font-mono text-[13px]">
            <Name name={t.name} />
          </span>
          <span className="text-[13px] text-fg-tertiary">{RISK_NAME[t.risk]}</span>
          {t.direct && (
            <Badge variant="outline" className="font-normal">
              direct
            </Badge>
          )}
        </div>
        <div className="line-clamp-2 text-[13px] wrap-break-word text-fg-secondary">{t.description}</div>
        {effectiveLine(t) && (
          <div data-effective={t.action ?? ''} className="text-[12px] wrap-break-word text-fg-tertiary">
            {effectiveLine(t)}
          </div>
        )}
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <AlwaysAskSelect name={t.name} session={session} />
        <SwitchGroup
          compact
          value={toolSwitch(t)}
          session={session}
          label={t.name}
          disabled={apply.isPending}
          onChange={(to) => apply.mutate(toolOps(category, t.name, to, session !== null))}
        />
      </div>
    </div>
  )
}

/** A builtin's effective state under its name: what a plain call does (the engine's answer for this chat, or harness-wide
 * in Settings), then the marked secret connections that still make it ask when it uses them. Null on an engine that
 * predates `action`. */
export function effectiveLine(t: Pick<BuiltinTool, 'action' | 'secret_asks'>): string | null {
  const base = t.action === 'allow' ? 'Runs without asking' : t.action === 'ask' ? 'Asks before running' : t.action === 'deny' ? 'Refused (policy or cap)' : null
  if (base === null) return null
  const secrets = t.secret_asks?.length ? `; asks when it uses ${alwaysAskSummary(t.secret_asks)} (always ask)` : ''
  return base + secrets
}

/** On / Off here / Off always. `mixed`: no item pressed. "Off here" needs the chat it applies to. */
function SwitchGroup({
  value,
  session,
  label,
  disabled,
  compact,
  onChange,
}: {
  value: BuiltinSwitch | 'mixed'
  session: string | null
  label: string
  disabled: boolean
  compact?: boolean
  onChange: (to: BuiltinSwitch) => void
}) {
  const item = compact
    ? 'h-7 min-w-7 px-1.5 text-xs pointer-coarse:min-h-11 pointer-coarse:min-w-11'
    : 'pointer-coarse:min-h-11 pointer-coarse:min-w-11'
  return (
    <span title={session === null ? CHAT_HINT : undefined} className="shrink-0">
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        aria-label={`${label}: on or off`}
        value={value === 'mixed' ? '' : value}
        disabled={disabled}
        onValueChange={(v) => v && onChange(v as BuiltinSwitch)}
      >
        <ToggleGroupItem value="on" className={item}>
          On
        </ToggleGroupItem>
        <ToggleGroupItem value="session" className={item} disabled={disabled || session === null}>
          Off here
        </ToggleGroupItem>
        <ToggleGroupItem value="always" className={item}>
          Off always
        </ToggleGroupItem>
      </ToggleGroup>
    </span>
  )
}
