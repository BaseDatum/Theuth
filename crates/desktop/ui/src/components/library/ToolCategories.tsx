import { useQuery } from '@tanstack/react-query'
import { useSearch } from '@tanstack/react-router'
import { ChevronRight, Pencil, Plus, Trash2, X } from 'lucide-react'
import { type FormEvent, useState } from 'react'
import { type Action, Empty, Section, errorText, useAction } from '@/components/settings/shared'
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
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { useDrafts } from '@/lib/drafts'
import { type HideOp, useMcpHide } from '@/lib/mcp-hide'
import { queryKeys, useApi } from '@/lib/rpc'
import { effectiveCategoryHidden, toolCategoryKey } from '@/lib/tool-categories'
import type { ToolCategoryInfo } from '@/rpc/bindings/method.agent.output'

const CATEGORY_KEYS = [['tool-categories']] as const
const ASSIGNED_VIEW_KEYS = [['tool-categories'], ['mcp'], ['builtins']] as const

type Save = { id: string | null; label: string; description: string | null }
type Assignment = { server: string; category: string | null }

type Actions = {
  save: Action<Save, ToolCategoryInfo>
  remove: Action<string, null>
  defaults: Action<string[], null>
  hidden: Action<HideOp[], null>
  assign: Action<Assignment, null>
}

/** Settings › Tools › Categories. The optional search parameter scopes the second switch to one chat. */
export function ToolCategoriesTab() {
  const session = useSearch({ from: '/settings/$tab', select: (s) => s.session ?? null })
  return <ToolCategories session={session} />
}

/** `session` is a chat id, `''` for the new-chat draft, or null for harness-wide category management only. */
export function ToolCategories({ session }: { session: string | null }) {
  const api = useApi()
  const draftHidden = useDrafts((s) => s.settings.hidden)
  const list = useQuery({
    queryKey: queryKeys.toolCategories(session),
    queryFn: () => api.list_tool_categories(session || null),
  })
  const hide = useMcpHide(session)
  const actions: Actions = {
    save: useAction(
      (v: Save) => api.upsert_tool_category(v.id, v.label.trim(), v.description?.trim() || null),
      { keys: ASSIGNED_VIEW_KEYS },
    ),
    remove: useAction((id: string) => api.delete_tool_category(id), { keys: ASSIGNED_VIEW_KEYS }),
    defaults: useAction((disabled: string[]) => api.set_default_tool_categories(disabled), { keys: CATEGORY_KEYS }),
    hidden: useAction(hide, { keys: ASSIGNED_VIEW_KEYS }),
    assign: useAction((v: Assignment) => api.set_mcp_category(v.server, v.category), { keys: ASSIGNED_VIEW_KEYS }),
  }
  const [label, setLabel] = useState('')
  const [description, setDescription] = useState('')

  const create = (e: FormEvent) => {
    e.preventDefault()
    if (!label.trim()) return
    actions.save.mutate(
      { id: null, label, description: description || null },
      {
        onSuccess: () => {
          setLabel('')
          setDescription('')
        },
      },
    )
  }

  const disabled = (list.data ?? []).filter((c) => !c.default_enabled).map((c) => c.id)
  return (
    <Section title="Tool categories">
      <form onSubmit={create} className="grid gap-2 border-b border-border-subtle pb-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)_auto]">
        <Input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="Category label"
          aria-label="New category label"
          disabled={actions.save.isPending}
        />
        <Input
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="Description (optional)"
          aria-label="New category description"
          disabled={actions.save.isPending}
        />
        <Button type="submit" size="sm" disabled={!label.trim() || actions.save.isPending}>
          <Plus />
          Create
        </Button>
      </form>
      {list.isPending && <p className="py-3 text-[13px] text-fg-tertiary">Loading…</p>}
      {list.error && <p className="py-2 text-[13px] break-words text-destructive">{errorText(list.error)}</p>}
      {list.data?.length === 0 && <Empty>No tool categories are available.</Empty>}
      {list.data?.map((category) => (
        <CategoryRow
          key={category.id}
          category={category}
          session={session}
          draftHidden={draftHidden}
          disabled={disabled}
          actions={actions}
        />
      ))}
    </Section>
  )
}

function CategoryRow({
  category: c,
  session,
  draftHidden,
  disabled,
  actions,
}: {
  category: ToolCategoryInfo
  session: string | null
  draftHidden: Record<string, boolean> | undefined
  disabled: string[]
  actions: Actions
}) {
  const [open, setOpen] = useState(false)
  const [editing, setEditing] = useState(false)
  const [label, setLabel] = useState(c.label)
  const [description, setDescription] = useState(c.description)
  const chatVisible = session === null ? null : !effectiveCategoryHidden(c, session, draftHidden)

  const changeDefault = (enabled: boolean) => {
    const next = enabled ? disabled.filter((id) => id !== c.id) : [...disabled, c.id]
    actions.defaults.mutate(next)
  }

  return (
    <Collapsible open={open} onOpenChange={setOpen} className="border-b border-border-subtle py-3 last:border-0">
      <CollapsibleTrigger asChild>
        <button type="button" className="flex w-full min-w-0 items-start gap-2 text-left pointer-coarse:min-h-11">
          <ChevronRight className={`mt-0.5 size-4 shrink-0 text-fg-tertiary transition-transform ${open ? 'rotate-90' : ''}`} />
          <span className="min-w-0 flex-1">
            <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span className="text-sm font-medium break-words">{c.label}</span>
              <span className="text-[13px] text-fg-tertiary">
                {c.builtin_count} tools{c.mcp_servers.length > 0 ? `, ${c.mcp_servers.length} MCP` : ''}
              </span>
              {c.custom && <Badge variant="outline" className="font-normal">Custom</Badge>}
            </span>
            {c.description && <span className="block text-[13px] wrap-break-word text-fg-secondary">{c.description}</span>}
          </span>
        </button>
      </CollapsibleTrigger>
      <div className="mt-2 ml-6 flex flex-wrap items-center gap-x-4 gap-y-2">
        <label className="flex items-center gap-2 text-[13px] text-fg-secondary">
          <Switch
            checked={c.default_enabled}
            disabled={actions.defaults.isPending}
            onCheckedChange={changeDefault}
            aria-label={`Default ${c.label} in new chats`}
          />
          Default in new chats
        </label>
        {session !== null && (
          <label className="flex items-center gap-2 text-[13px] text-fg-secondary">
            <input
              type="checkbox"
              className="size-4 accent-primary"
              checked={chatVisible ?? false}
              disabled={actions.hidden.isPending}
              onChange={(e) =>
                actions.hidden.mutate([{ names: [toolCategoryKey(c.id)], hidden: !e.currentTarget.checked }])
              }
              aria-label={`${c.label} in this chat`}
            />
            This chat
          </label>
        )}
        {c.custom && (
          <span className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              aria-label={`Edit ${c.label}`}
              disabled={actions.save.isPending}
              onClick={() => {
                setLabel(c.label)
                setDescription(c.description)
                setEditing(true)
              }}
            >
              <Pencil />
              Edit
            </Button>
            <AlertDialog>
              <AlertDialogTrigger asChild>
                <Button variant="ghost" size="sm" aria-label={`Delete ${c.label}`} disabled={actions.remove.isPending}>
                  <Trash2 />
                  Delete
                </Button>
              </AlertDialogTrigger>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>Delete {c.label}?</AlertDialogTitle>
                  <AlertDialogDescription>
                    Its MCP servers and built-in tool overrides become unassigned. This cannot be undone.
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>Cancel</AlertDialogCancel>
                  <AlertDialogAction variant="destructive" onClick={() => actions.remove.mutate(c.id)}>
                    Delete category
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          </span>
        )}
      </div>
      {c.custom && editing && (
        <form
          className="mt-3 ml-6 grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)_auto_auto]"
          onSubmit={(e) => {
            e.preventDefault()
            if (!label.trim()) return
            actions.save.mutate(
              { id: c.id, label, description: description || null },
              { onSuccess: () => setEditing(false) },
            )
          }}
        >
          <Input value={label} onChange={(e) => setLabel(e.target.value)} aria-label={`Edit ${c.label} label`} disabled={actions.save.isPending} />
          <Input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Description (optional)"
            aria-label={`Edit ${c.label} description`}
            disabled={actions.save.isPending}
          />
          <Button type="submit" size="sm" disabled={!label.trim() || actions.save.isPending}>Save</Button>
          <Button type="button" variant="ghost" size="sm" onClick={() => setEditing(false)} disabled={actions.save.isPending}>Cancel</Button>
        </form>
      )}
      <CollapsibleContent className="mt-3 ml-6 space-y-3">
        <div>
          <p className="mb-1 text-[13px] font-medium text-fg-secondary">MCP servers</p>
          {c.mcp_servers.length === 0 ? (
            <p className="text-[13px] text-fg-tertiary">No MCP servers assigned.</p>
          ) : (
            <ul className="space-y-1">
              {c.mcp_servers.map((server) => (
                <li key={server} className="flex min-w-0 items-center gap-2">
                  <code className="min-w-0 flex-1 text-[13px] break-words">{server}</code>
                  <Button
                    variant="ghost"
                    size="xs"
                    aria-label={`Unassign ${server} from ${c.label}`}
                    disabled={actions.assign.isPending}
                    onClick={() => actions.assign.mutate({ server, category: null })}
                  >
                    <X />
                    Unassign
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div>
          <p className="mb-1 text-[13px] font-medium text-fg-secondary">Built-in tools</p>
          {c.builtin_tools.length === 0 ? (
            <p className="text-[13px] text-fg-tertiary">No built-in tools in this category.</p>
          ) : (
            <ul className="flex flex-wrap gap-x-3 gap-y-1">
              {c.builtin_tools.map((tool) => <li key={tool}><code className="text-[13px] break-words">{tool}</code></li>)}
            </ul>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
}
