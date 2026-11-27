import { useQuery } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { useId, useState } from 'react'
import { Name } from '@/components/library/Tools'
import { fromInfo, setterField } from '@/components/settings/copy/fields'
import { Empty, Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { alwaysAskSummary, serverKey, setAlwaysAsk, useAlwaysAsk } from '@/lib/always-ask'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { askKeysOf, parseAskKey } from '@/components/library/secrets/labels'
import { useConnection } from '@/store/connection'

/** 188 A10: Settings › Agent › Approvals — the global default only; a chat's override and the waiting queue live with the chat. */
export function Approvals() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const setGlobal = useAction((on: boolean) => api.set_approvals(null, on).then(() => refreshInfo(api)))
  const always = useQuerySupport(['get_always_ask', 'set_always_ask'])
  const off = info?.approvals.global === false
  return (
    <>
      <Section title="Approvals">
        <Row
          label="Ask before risky tools"
          copy={setterField('approvals', 'Ask before risky tools', info?.approvals.global, 'set_approvals', { args: (on) => [null, on], read: fromInfo((i) => i.approvals.global) })}
          description={
            off
              ? 'Off: tools run without asking, except the always-ask entries below and the mandatory gates. Each chat can override this from its menu.'
              : 'The default for every chat. Each chat can override it from its menu.'
          }
        >
          <Switch checked={info?.approvals.global ?? false} disabled={!info || setGlobal.isPending} onCheckedChange={(v) => setGlobal.mutate(v)} />
        </Row>
      </Section>
      {always && <AlwaysAsk />}
    </>
  )
}

const SECRET_DOING: Record<string, string> = {
  read: 'reads a secret through this connection',
  write: 'creates or updates an item in this connection',
  delete: 'deletes an item in this connection',
  send: 'creates a Send from this connection',
}

/** What one entry makes ask: a `secret:` key covers every tool that uses the connection (`shell` with a `{{secret:…}}`
 * reference included), `mcp:<server>` every tool of the server, anything else the one tool. */
export function entryScope(entry: string): string {
  const k = parseAskKey(entry)
  if (k) return `Any tool that ${SECRET_DOING[k.dir] ?? k.dir}, shell commands with a {{secret:…}} reference included.`
  if (entry.startsWith('mcp:')) return `Every tool of the MCP server ${entry.slice(4)}.`
  return `The ${entry} tool.`
}

/** 219 §6.1: the harness-wide always-ask list (`tools.always_ask`): one row per entry with what it covers and its remove
 * control, and an add field that suggests built-in tools, MCP tools and `mcp:<server>` for a whole server. */
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
  const secrets = useApi()
  const canConns = useQuerySupport(['list_secret_connections'])
  const conns = useQuery({ enabled: canConns, queryKey: queryKeys.secretConnections, queryFn: () => secrets.list_secret_connections() })
  const suggestions = [
    ...new Set([
      ...(builtins.data ?? []).flatMap((c) => c.tools.map((t) => t.name)),
      ...(mcp.data ?? []).flatMap((s) => [serverKey(s.name), ...s.tools.map((t) => t.registered ?? t.name)]),
      ...askKeysOf(conns.data ?? []),
    ]),
  ].filter((n) => !state?.global.includes(n))
  const set = useAction((v: { tool: string; on: boolean }) => setAlwaysAsk(api, null, v.tool, v.on), { keys: [['always-ask'], queryKeys.secretConnections] })
  const add = () => {
    const tool = name.trim()
    if (tool) set.mutate({ tool, on: true }, { onSuccess: () => setName('') })
  }
  const global = state?.global ?? []
  return (
    <Section title="Always ask, even when approvals are off">
      <p className="py-2 text-[13px] text-fg-secondary">
        These entries always need your approval, in every chat, routine and subagent. The double never answers them. They are
        stored in config.yaml under <span className="font-mono">tools.always_ask</span>; a chat's own entries live in its Tools sheet.
      </p>
      {global.length === 0 && <Empty>No tool always asks.</Empty>}
      {global.length > 0 && (
        <p data-always-ask-summary="" className="pb-2 text-[13px]">
          Always ask is on for {global.length === 1 ? '1 entry' : `${global.length} entries`}: {alwaysAskSummary(global)}.
        </p>
      )}
      {global.map((tool) => (
        <Row key={tool} label={<span className="font-mono text-[13px]"><Name name={tool} /></span>} description={entryScope(tool)}>
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
      <p className="pb-2 text-[12px] text-fg-tertiary">
        Mandatory gates ask whatever the switches say: owner-only tools, writes to theuth's own config files, and adding or
        removing MCP servers while any entry above exists. Their cards say so.
      </p>
    </Section>
  )
}
