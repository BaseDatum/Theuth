import { useState } from 'react'
import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, Radar, Trash2 } from 'lucide-react'
import { notify as toast } from '@/lib/notify'
import { Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import type { EntryInfo } from '@/rpc/bindings/method.agent.output'

const LOGIN_PREFIX = 'login required: '

export function Agents() {
  const canQuery1 = useQuerySupport(["list_acp"])
  const canQuery1_2 = useQuerySupport(["list_acp_registry"])
  const api = useApi()
  const qc = useQueryClient()
  const installed = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.acp,
    queryFn: () => api.list_acp(false),
  })
  const registry = useQuery({ enabled: canQuery1_2,
    queryKey: queryKeys.acpRegistry,
    queryFn: () => api.list_acp_registry(),
  })

  // `probe: true` runs `initialize` against each harness; its reply is the fresher list.
  const probe = useAction((_: void) => api.list_acp(true), {
    onSuccess: (list) => qc.setQueryData(queryKeys.acp, list),
  })
  const add = useAction((id: string) => api.add_acp(id), {
    keys: [queryKeys.acp],
    onSuccess: (e) => toast.success(`Added ${e.name}`),
  })
  const remove = useAction((name: string) => api.remove_acp(name), {
    keys: [queryKeys.acp],
  })
  const login = useAction(
    ({ name, method }: { name: string; method: string | null }) => api.acp_login(name, method),
    {
      onSuccess: async (status, { name }) => {
        qc.setQueryData<EntryInfo[]>(queryKeys.acp, (list) =>
          list?.map((e) => (e.name === name ? { ...e, status } : e)),
        )
        if (status === 'ok') toast.success(`Logged in to ${name}`)
        else toast.warning(`${name}: ${status}`)
        try {
          qc.setQueryData(queryKeys.acp, await api.list_acp(true))
        } catch {
          // Keep the status returned by acp_login if re-probing all harnesses fails.
        }
      },
    },
  )

  return (
    <>
      <Section
        title="Installed"
        actions={
          <Button
            variant="ghost"
            size="sm"
            disabled={probe.isPending || login.isPending}
            onClick={() => probe.mutate(undefined)}
          >
            <Radar className={probe.isPending ? 'animate-pulse' : undefined} />
            Probe
          </Button>
        }
      >
        {installed.data?.length === 0 && (
          <Empty>No harness configured. Add one from the registry below.</Empty>
        )}
        {installed.data?.map((e) => (
          <HarnessRow
            key={e.name}
            entry={e}
            probing={probe.isPending}
            removing={remove.isPending}
            loggingIn={login.isPending && login.variables?.name === e.name}
            loginBusy={login.isPending}
            onLogin={(name, method) => login.mutate({ name, method })}
            onRemove={(name) => remove.mutate(name)}
          />
        ))}
      </Section>
      <Section title="Registry">
        {registry.data?.length === 0 && <Empty>The registry is empty.</Empty>}
        {registry.data?.map((a) => (
          <Row key={a.id} label={`${a.name} ${a.version}`} description={a.description}>
            <span className="font-mono text-[13px] text-fg-tertiary">{a.id}</span>
            <Button
              variant="outline"
              size="sm"
              disabled={add.isPending}
              onClick={() => add.mutate(a.id)}
            >
              <Plus />
              Add
            </Button>
          </Row>
        ))}
      </Section>
    </>
  )
}

function HarnessRow({
  entry: e,
  probing,
  removing,
  loggingIn,
  loginBusy,
  onLogin,
  onRemove,
}: {
  entry: EntryInfo
  probing: boolean
  removing: boolean
  loggingIn: boolean
  loginBusy: boolean
  onLogin: (name: string, method: string | null) => void
  onRemove: (name: string) => void
}) {
  const loginDetail =
    !probing && e.status !== null && e.status.startsWith(LOGIN_PREFIX)
      ? e.status.slice(LOGIN_PREFIX.length)
      : null
  const needsLogin = loginDetail !== null
  const agentMethods = (e.auth_methods ?? []).filter((m) => m.kind === 'agent')
  const [picked, setPicked] = useState<string | null>(null)
  const selectedMethod = agentMethods.some((m) => m.id === picked) ? picked : (agentMethods[0]?.id ?? null)

  return (
    <Row
      label={e.name}
      description={
        <>
          <span className="font-mono">{[e.command, ...e.args].join(' ')}</span>
          {e.description && <> · {e.description}</>}
        </>
      }
    >
      {needsLogin && agentMethods.length === 0 ? (
        <span className="flex items-center gap-1.5 text-[13px] text-fg-secondary">
          <Dot on={false} />
          <span>
            Login required — run{' '}
            <code className="select-all rounded bg-layer-2 px-1 py-0.5 font-mono text-xs text-fg-primary">
              {loginDetail}
            </code>
          </span>
        </span>
      ) : (
        <span className="flex items-center gap-1.5 text-[13px] text-fg-secondary">
          {!probing && e.status !== null && <Dot on={e.status === 'ok'} />}
          {probing ? 'probing…' : e.status}
        </span>
      )}
      {needsLogin && agentMethods.length > 1 && (
        <select
          aria-label={`Auth method for ${e.name}`}
          value={selectedMethod ?? ''}
          disabled={loginBusy || probing}
          onChange={(ev) => setPicked(ev.target.value)}
          className="h-8 rounded-md border border-border-subtle bg-transparent px-2 text-base md:text-sm"
        >
          {agentMethods.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
        </select>
      )}
      {needsLogin && agentMethods.length > 0 && (
        <Button
          variant="outline"
          size="sm"
          disabled={loginBusy || probing}
          onClick={() => onLogin(e.name, selectedMethod)}
        >
          {loggingIn ? 'Waiting for browser…' : 'Log in'}
        </Button>
      )}
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={`Remove ${e.name}`}
        disabled={removing || loggingIn}
        onClick={() => onRemove(e.name)}
      >
        <Trash2 />
      </Button>
    </Row>
  )
}
