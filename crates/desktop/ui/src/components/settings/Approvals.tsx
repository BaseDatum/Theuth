import { useQuery } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { useId, useState } from 'react'
import { Name } from '@/components/library/Tools'
import { Empty, Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { serverKey, setAlwaysAsk, useAlwaysAsk } from '@/lib/always-ask'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { secretKeys, useSecretsApi, useSecretsSupport } from '@/components/library/secrets/api'
import { askKeysOf } from '@/components/library/secrets/labels'
import { useConnection } from '@/store/connection'

/** 188 A10: Settings › Agent › Approvals — the global default only; a chat's override and the waiting queue live with the chat. */
export function Approvals() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const setGlobal = useAction((on: boolean) => api.set_approvals(null, on).then(() => refreshInfo(api)))
  const always = useQuerySupport(['get_always_ask', 'set_always_ask'])
  return (
    <>
      <Section title="Approvals">
        <Row label="Ask before risky tools" description="The default for every chat. Each chat can override it from its menu.">
          <Switch checked={info?.approvals.global ?? false} disabled={!info || setGlobal.isPending} onCheckedChange={(v) => setGlobal.mutate(v)} />
        </Row>
      </Section>
      {always && <AlwaysAsk />}
    </>
  )
}

/** 219 §6.1: the harness-wide always-ask list (`tools.always_ask`): one row per entry with its remove control, and an add
 * field that suggests built-in tools, MCP tools and `mcp:<server>` for a whole server. */
function AlwaysAsk() {
  const api = useApi()
  const state = useAlwaysAsk(null)
  const [name, setName] = useState('')
  const listId = useId()
  const canBuiltins = useQuerySupport(['list_builtins'])
  const canMcp = useQuerySupport(['list_mcp'])
  const builtins = useQuery({ enabled: canBuiltins, queryKey: queryKeys.builtins(null), queryFn: () => api.list_builtins(null) })
  const mcp = useQuery({ enabled: canMcp, queryKey: queryKeys.mcp(null), queryFn: () => api.list_mcp(null) })
  // 221 §8: each connection's `secret:<backend>/<conn>:<direction>` keys, so a removed one can be put back.
  const secrets = useSecretsApi()
  const canConns = useSecretsSupport(['list_secret_connections'])
  const conns = useQuery({ enabled: canConns, queryKey: secretKeys.connections, queryFn: () => secrets.list_secret_connections() })
  const suggestions = [
    ...new Set([
      ...(builtins.data ?? []).flatMap((c) => c.tools.map((t) => t.name)),
      ...(mcp.data ?? []).flatMap((s) => [serverKey(s.name), ...s.tools.map((t) => t.registered ?? t.name)]),
      ...askKeysOf(conns.data ?? []),
    ]),
  ].filter((n) => !state?.global.includes(n))
  const set = useAction((v: { tool: string; on: boolean }) => setAlwaysAsk(api, null, v.tool, v.on), { keys: [['always-ask'], secretKeys.connections] })
  const add = () => {
    const tool = name.trim()
    if (tool) set.mutate({ tool, on: true }, { onSuccess: () => setName('') })
  }
  return (
    <Section title="Always ask, even when approvals are off">
      <p className="py-2 text-[13px] text-fg-secondary">
        These tools always need your approval, in every chat, routine and subagent. The double never answers them.
      </p>
      {state?.global.length === 0 && <Empty>No tool always asks.</Empty>}
      {state?.global.map((tool) => (
        <Row key={tool} label={<span className="font-mono text-[13px]"><Name name={tool} /></span>}>
          <Button variant="ghost" size="icon-sm" aria-label={`Stop always asking for ${tool}`} disabled={set.isPending} onClick={() => set.mutate({ tool, on: false })}>
            <X />
          </Button>
        </Row>
      ))}
      <div className="flex items-center gap-2 py-3">
        <Input
          list={listId}
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && add()}
          placeholder="Tool name, mcp:<server> or secret:<backend>/<conn>:<direction>"
          aria-label="Tool to always ask for"
          spellCheck={false}
          className="h-8 font-mono text-base md:text-xs"
        />
        <datalist id={listId}>
          {suggestions.map((n) => (
            <option key={n} value={n} />
          ))}
        </datalist>
        <Button size="sm" variant="outline" disabled={!name.trim() || set.isPending} onClick={add}>
          Add
        </Button>
      </div>
    </Section>
  )
}
