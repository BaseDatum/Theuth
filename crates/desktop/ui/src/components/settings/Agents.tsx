import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, Radar, Trash2 } from 'lucide-react'
import { notify as toast } from '@/lib/notify'
import { Dot, Empty, Row, Section, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'

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

  return (
    <>
      <Section
        title="Installed"
        actions={
          <Button
            variant="ghost"
            size="sm"
            disabled={probe.isPending}
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
          <Row
            key={e.name}
            label={e.name}
            description={
              <>
                <span className="font-mono">{[e.command, ...e.args].join(' ')}</span>
                {e.description && <> · {e.description}</>}
              </>
            }
          >
            <span className="flex items-center gap-1.5 text-[13px] text-fg-secondary">
              {!probe.isPending && e.status !== null && <Dot on={e.status === 'ok'} />}
              {probe.isPending ? 'probing…' : e.status}
            </span>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Remove ${e.name}`}
              disabled={remove.isPending}
              onClick={() => remove.mutate(e.name)}
            >
              <Trash2 />
            </Button>
          </Row>
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
