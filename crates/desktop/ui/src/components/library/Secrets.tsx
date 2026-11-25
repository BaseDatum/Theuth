import { useQuery } from '@tanstack/react-query'
import { Column, Page } from '@/components/library/Page'
import { secretKeys, useSecretsApi, useSecretsSupport } from '@/components/library/secrets/api'
import { Connections } from '@/components/library/secrets/Connections'
import { LocalList } from '@/components/library/secrets/LocalList'
import { Section } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'

/**
 * `/secrets`: 221 §9.1's Connections (external managers: browse, Reveal, Delete / Restore, Sends, settings) above 37 §6's
 * local vault list. On an engine without 221 the Connections section is absent and the page is 37's.
 */
export function Secrets() {
  const api = useApi()
  const secrets = useSecretsApi()
  const canLocal = useQuerySupport(['list_secrets'])
  const canConns = useSecretsSupport(['list_secret_connections'])
  const canSend = useSecretsSupport(['create_send'])
  const list = useQuery({ enabled: canLocal, queryKey: queryKeys.secrets, queryFn: () => api.list_secrets() })
  const conns = useQuery({ enabled: canConns, queryKey: secretKeys.connections, queryFn: () => secrets.list_secret_connections() })
  return (
    <Page actions={list.data && <span className="px-2 text-[13px] text-fg-tertiary">key: {list.data.key_store}</span>}>
      <Column>
        <Connections />
        {canConns ? (
          <Section title="Local vault">
            <LocalList canSend={canSend && (conns.data ?? []).some((c) => c.backend === 'bw')} />
          </Section>
        ) : (
          <LocalList canSend={false} />
        )}
      </Column>
    </Page>
  )
}
