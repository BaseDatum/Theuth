// 158 §12.6 (W5): Settings › MCP server — the on/off switch (`mcp_server_set`), status line, port (default 7788),
// Token with Rotate (`mcp_server_rotate_token`), "Copy MCP config" per client (`lib/mcp-config`), the last 20 audit
// lines (`mcp_server_audit`) and the lawful "What is shared" sentence. 188 (D2): the servers theuth uses, with their home
// machines and logins, are on their rows on Agents › Tools.

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { toast } from 'sonner'
import { Dot, Empty, Row, Section, bounded, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { MCP_CLIENTS, MCP_CLIENT_LABEL, MCP_SHARED_LINE, type McpClient, mcpActions, mcpConfig } from '@/lib/mcp-config'
import { useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import { useConnection } from '@/store/connection'

export const ready = true

const KEYS = { status: ['mcp-server'] as const, audit: ['mcp-server', 'audit'] as const }

export function Mcp() {
  const api = useApi()
  const dataDir = useConnection((s) => s.info?.data_dir ?? '<data_dir>')
  const status = useQuery({ queryKey: KEYS.status, queryFn: () => api.mcp_server_get() })
  const audit = useQuery({ queryKey: KEYS.audit, queryFn: () => api.mcp_server_audit(20) })
  const [client, setClient] = useState<McpClient>('claude')
  const acts = mcpActions(api)
  const setEnabled = useAction((on: boolean) => acts.setEnabled(status.data!, on), { keys: [KEYS.status] })
  const setPort = useAction((port: number) => acts.setPort(status.data!, port), { keys: [KEYS.status] })
  const rotate = useAction(() => acts.rotate(), {
    keys: [KEYS.status],
    onSuccess: () => toast.success('Token rotated; every open MCP session ended'),
  })

  if (status.isError) return <Empty>{errorText(status.error)}</Empty>
  const s = status.data
  if (!s) return <Empty>Loading…</Empty>
  const busy = setEnabled.isPending || setPort.isPending

  return (
    <>
      <Section title="MCP server">
        <Row
          label="theuth as an MCP server"
          description={
            <span className="flex items-center gap-1.5">
              <Dot on={s.running} />
              {s.running ? `running on ${s.url}` : s.enabled ? 'starting…' : 'off'}
            </span>
          }
        >
          <Switch checked={s.enabled} disabled={busy} onCheckedChange={(on) => setEnabled.mutate(on)} />
        </Row>
        <Row label="Port" description="Loopback only (127.0.0.1); RPC is 7777.">
          <Input
            key={s.port}
            type="number"
            min={1024}
            max={65535}
            className="w-24"
            aria-label="MCP port"
            defaultValue={s.port}
            disabled={busy}
            onBlur={(e) => {
              const port = bounded(e.target.value, 1024, 65535)
              if (port !== undefined && port !== s.port) setPort.mutate(port)
            }}
          />
        </Row>
        <Row label="Token" description="Sent as Authorization: Bearer on every request. Rotating ends every open MCP session.">
          <code className="text-[13px] text-fg-secondary">{s.token ? `${s.token.slice(0, 8)}…` : 'not generated yet'}</code>
          <Button size="sm" variant="outline" disabled={rotate.isPending} onClick={() => rotate.mutate()}>
            Rotate
          </Button>
        </Row>
        <Row label="Copy MCP config" description="The block your client reads; the token is filled in.">
          <Select value={client} onValueChange={(v) => setClient(v as McpClient)}>
            <SelectTrigger className="w-40" aria-label="MCP client">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {MCP_CLIENTS.map((c) => (
                <SelectItem key={c} value={c}>
                  {MCP_CLIENT_LABEL[c]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            size="sm"
            onClick={() =>
              navigator.clipboard
                .writeText(mcpConfig(client, s, dataDir))
                .then(() => toast.message(`Copied ${MCP_CLIENT_LABEL[client]} config`), () => toast.error('Copy failed'))
            }
          >
            Copy
          </Button>
        </Row>
      </Section>
      <Section title="Recent calls">
        {audit.isError ? (
          <Empty>{errorText(audit.error)}</Empty>
        ) : !audit.data ? (
          <Empty>Loading…</Empty>
        ) : audit.data.length === 0 ? (
          <Empty>No MCP calls yet.</Empty>
        ) : (
          <table className="w-full text-[13px]">
            <tbody>
              {audit.data.map((l) => (
                <tr key={`${l.at}-${l.tool}-${l.args_sha256}`} className="border-b border-border-subtle last:border-0">
                  <td className="py-1 pr-3 whitespace-nowrap text-fg-tertiary">{formatAt(Date.parse(l.at))}</td>
                  <td className="py-1 pr-3 text-fg-secondary">{l.client}</td>
                  <td className="py-1 pr-3 font-mono">{l.tool}</td>
                  <td className="py-1 pr-3 text-fg-tertiary">{l.transport}</td>
                  <td className={`py-1 pr-3 ${l.ok ? '' : 'text-destructive'}`}>{l.ok ? 'ok' : 'error'}</td>
                  <td className="py-1 text-right text-fg-tertiary">{l.ms} ms</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Section>
      <p className="text-[13px] text-fg-secondary">{MCP_SHARED_LINE}</p>
    </>
  )
}
