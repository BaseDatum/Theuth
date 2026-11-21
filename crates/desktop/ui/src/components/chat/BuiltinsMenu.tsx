import { useQuery } from '@tanstack/react-query'
import { useAction } from '@/components/settings/shared'
import { type BuiltinCategory, categoryState, sessionCategoryOps, toolSwitch, useBuiltinsApply, withDraftHides } from '@/lib/builtins'
import { useDrafts } from '@/lib/drafts'
import { queryKeys, useApi } from '@/lib/rpc'
import { MenuCheck } from './MenuCheck'

/** 206: the composer popover's Builtins panel, one checkbox per builtin category for this chat (`''`: the new-chat
 * draft). It writes only the chat's hides (`cat:<id>` and tool names through `set_mcp_hidden`), never the
 * harness-wide set: a category that is off always is greyed, the way there in its title. */
export function BuiltinsMenu({ session, enabled }: { session: string; enabled: boolean }) {
  const api = useApi()
  const draftHidden = useDrafts((s) => s.settings.hidden)
  const categories = useQuery({
    enabled,
    queryKey: queryKeys.builtins(session),
    queryFn: () => api.list_builtins(session || null),
  })
  // Through `useBuiltinsApply`, as the sheet's switches: it refetches every builtins list before the row re-enables.
  const apply = useBuiltinsApply(session)
  const toggle = useAction((v: { category: BuiltinCategory; hidden: boolean }) => apply({ session: sessionCategoryOps(v.category, v.hidden), always: [] }))
  const rows = session === '' ? withDraftHides(categories.data ?? [], draftHidden) : (categories.data ?? [])
  return (
    <div role="menu" aria-label="Builtin tools for this chat" className="max-h-72 overflow-y-auto">
      {categories.isPending && <p className="px-3 py-2 text-sm text-fg-tertiary">Loading…</p>}
      {categories.error && <p className="px-3 py-2 text-sm break-words text-destructive">{categories.error.message}</p>}
      {categories.data?.length === 0 && <p className="px-3 py-2 text-sm text-fg-tertiary">No builtin tools.</p>}
      {rows.map((c) => {
        const always = c.tools.every((t) => t.disabled)
        const state = categoryState(c)
        const on = c.tools.filter((t) => toolSwitch(t) === 'on').length
        return (
          <MenuCheck
            key={c.id}
            label={c.label}
            state={state}
            live={!always}
            busy={toggle.isPending}
            title={always ? 'Off always (Settings › Tools › Builtins)' : c.description}
            count={on === 0 ? 'off' : `${on}/${c.tools.length}`}
            onToggle={() => toggle.mutate({ category: c, hidden: state === 'on' })}
          />
        )
      })}
    </div>
  )
}
