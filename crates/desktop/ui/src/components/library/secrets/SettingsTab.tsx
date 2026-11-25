import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { useState } from 'react'
import { Empty, Row, Section } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { secretKeys, useSecretsApi, useSecretsSupport } from './api'
import { AskSwitches, ConnSettingsPanel, ExternalSettingsPanel } from './ConnSettings'
import { BACKEND_LABEL, statusLabel } from './labels'

/**
 * 221 §8 Settings › Agent › Secrets: the harness-wide D20 keys (Send file size, Copy link needs a press), and each
 * connection's ask switches and Settings disclosure — the same controls as the Secrets page, which adds and removes
 * connections.
 */
export function SecretsSettingsTab() {
  const api = useSecretsApi()
  const can = useSecretsSupport(['list_secret_connections', 'set_connection_setting'])
  const conns = useQuery({ enabled: can, queryKey: secretKeys.connections, queryFn: () => api.list_secret_connections() })
  const [open, setOpen] = useState<string | null>(null)
  return (
    <>
      <Section title="External secrets">
        <ExternalSettingsPanel />
      </Section>
      {can && (
        <Section
          title="Connections"
          actions={
            <Button variant="outline" size="sm" asChild>
              <Link to="/secrets">Secrets page</Link>
            </Button>
          }
        >
          {conns.data?.length === 0 && <Empty>No connection. Add AWS Secrets Manager or Bitwarden on the Secrets page.</Empty>}
          {conns.data?.map((c) => (
            <div key={c.conn} className="border-b border-border-subtle last:border-0">
              <Row
                wide
                label={
                  <>
                    <span className="font-mono">{c.conn}</span> <span className="text-fg-tertiary">· {BACKEND_LABEL[c.backend]} · {statusLabel(c.status)}</span>
                  </>
                }
                description={c.summary}
              >
                <AskSwitches info={c} />
                <Button variant="ghost" size="sm" className="ml-auto" aria-expanded={open === c.conn} onClick={() => setOpen(open === c.conn ? null : c.conn)}>
                  Settings
                </Button>
              </Row>
              {open === c.conn && <ConnSettingsPanel info={c} />}
            </div>
          ))}
        </Section>
      )}
    </>
  )
}
