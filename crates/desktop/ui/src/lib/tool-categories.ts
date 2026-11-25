import type { ToolCategoryInfo } from '@/rpc/bindings/method.agent.output'

/** The `set_mcp_hidden` entry naming a high-level tool category. */
export const toolCategoryKey = (id: string): string => `tcat:${id}`

/** Whether a category is hidden in a live chat or in the not-yet-created chat's draft. */
export function effectiveCategoryHidden(cat: ToolCategoryInfo, session: string, draftHidden?: Record<string, boolean>): boolean {
  if (session !== '') return cat.hidden
  return draftHidden?.[toolCategoryKey(cat.id)] ?? !cat.default_enabled
}
