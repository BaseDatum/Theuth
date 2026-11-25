import { useAction } from '@/components/settings/shared'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { type AlwaysAskLevel, alwaysAskOf, setAlwaysAskLevel, useAlwaysAsk } from '@/lib/always-ask'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'

/** The select's value for an entry an ancestor chat set: shown, never picked. */
const INHERITED = 'inherited'

/**
 * 219 §6.1: a tool's (or, with `mcp:<server>`, a server's) always-ask control on the Builtins and MCP rows: off, this
 * chat, or always (harness-wide). "This chat" needs the chat (`session`, `''` the draft); an entry inherited from a
 * parent chat reads `this chat (inherited)` and cannot change here. Hidden on an engine without the methods.
 */
export function AlwaysAskSelect({ name, session, disabled }: { name: string; session: string | null; disabled?: boolean }) {
  const can = useQuerySupport(['get_always_ask', 'set_always_ask'])
  const api = useApi()
  const state = useAlwaysAsk(session, can)
  const set = useAction((to: AlwaysAskLevel) => setAlwaysAskLevel(api, session, state, name, to), { keys: [['always-ask']] })
  if (!can) return null
  const { level, inherited } = alwaysAskOf(state, name)
  return (
    <Select
      value={inherited ? INHERITED : level}
      disabled={disabled || inherited || !state || set.isPending}
      onValueChange={(v) => v !== INHERITED && set.mutate(v as AlwaysAskLevel)}
    >
      <SelectTrigger size="sm" className="w-auto min-w-36" aria-label={`Always ask for ${name}`}>
        <span className="text-fg-tertiary">Always ask</span>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="off">off</SelectItem>
        {session !== null && <SelectItem value="session">this chat</SelectItem>}
        {inherited && <SelectItem value={INHERITED}>this chat (inherited)</SelectItem>}
        <SelectItem value="always">always</SelectItem>
      </SelectContent>
    </Select>
  )
}
