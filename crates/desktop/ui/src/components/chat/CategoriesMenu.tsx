import { useQuery } from '@tanstack/react-query'
import { useAction } from '@/components/settings/shared'
import { useDrafts } from '@/lib/drafts'
import { useMcpHide } from '@/lib/mcp-hide'
import { queryKeys, useApi } from '@/lib/rpc'
import { effectiveCategoryHidden, toolCategoryKey } from '@/lib/tool-categories'
import type { ToolCategoryInfo } from '@/rpc/bindings/method.agent.output'
import { MenuCheck } from './MenuCheck'

/** High-level tool categories for this chat (`''`: the new-chat draft). */
export function CategoriesMenu({ session, enabled }: { session: string; enabled: boolean }) {
  const api = useApi()
  const draftHidden = useDrafts((s) => s.settings.hidden)
  const categories = useQuery({
    enabled,
    queryKey: queryKeys.toolCategories(session),
    queryFn: () => api.list_tool_categories(session || null),
  })
  const hide = useMcpHide(session)
  const toggle = useAction(
    (v: { category: ToolCategoryInfo; hidden: boolean }) => hide([{ names: [toolCategoryKey(v.category.id)], hidden: v.hidden }]),
    { keys: [['builtins'], ['mcp']] },
  )
  return (
    <div role="menu" aria-label="Tool categories for this chat" className="max-h-72 overflow-y-auto">
      {categories.isPending && <p className="px-3 py-2 text-sm text-fg-tertiary">Loading…</p>}
      {categories.error && <p className="px-3 py-2 text-sm break-words text-destructive">{categories.error.message}</p>}
      {categories.data?.length === 0 && <p className="px-3 py-2 text-sm text-fg-tertiary">No tool categories.</p>}
      {categories.data?.map((category) => {
        const hidden = effectiveCategoryHidden(category, session, draftHidden)
        const mcp = category.mcp_servers.length
        return (
          <MenuCheck
            key={category.id}
            label={category.label}
            state={hidden ? 'off' : 'on'}
            live
            busy={toggle.isPending}
            title={category.description}
            count={`${category.builtin_count} tools${mcp > 0 ? ` · ${mcp} MCP` : ''}`}
            detail={<span className="block truncate text-xs text-fg-tertiary">{category.description}</span>}
            onToggle={() => toggle.mutate({ category, hidden: !hidden })}
          />
        )
      })}
    </div>
  )
}
