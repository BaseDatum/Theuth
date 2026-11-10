import { useQuery } from '@tanstack/react-query'
import { useModels } from '@/components/agents/queries'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { SAME_AS_NEW } from '@/components/settings/Chat'
import { Row, Section, useAction } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'

/** 188 A4: Settings › Models › Defaults — the model defaults (148, engine settings): new sessions, native subagents, the theuth agent. */
export function ModelDefaults() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const models = useModels()
  const subagentModel = useQuery({ queryKey: queryKeys.subagentModel, queryFn: () => api.subagent_model() })
  const theuthModel = useQuery({ queryKey: queryKeys.theuthAgentModel, queryFn: () => api.theuth_agent_model() })
  // R1: `session: null` + `manual` sets the default; `info` follows through `DefaultModelChanged` / `DefaultEffortChanged`.
  const setDefaultModel = useAction((id: string) => api.set_model(null, id, true))
  const setDefaultEffort = useAction((level: string | null) => api.set_effort(null, level, true))
  const setSubagentModel = useAction((id: string | null) => api.set_subagent_model(id), { keys: [queryKeys.subagentModel] })
  // R3: the roster synthesises theuth's `AgentFile.model` from this, so the Agents page refetches too.
  const setTheuthModel = useAction((id: string | null) => api.set_theuth_agent_model(id), { keys: [queryKeys.theuthAgentModel, queryKeys.roster] })
  const current = models.data?.find((m) => m.id === info?.default_model)
  return (
    <Section title="Models">
      <Row stack label="New sessions" description="The model every new chat starts on. Picking a model inside a chat changes only that chat.">
        <ModelCombobox
          aria-label="New sessions model"
          className="max-w-64 max-md:w-full max-md:max-w-none"
          models={models.data ?? []}
          value={info?.default_model}
          effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: info?.default_effort, onSelect: (level) => setDefaultEffort.mutate(level) } : undefined}
          disabled={!info || setDefaultModel.isPending || setDefaultEffort.isPending}
          onChange={(v) => v && setDefaultModel.mutate(v)}
        />
      </Row>
      <Row
        stack
        label="Subagents"
        description="What theuth subagents start on unless you ask the agent for a specific model. External harnesses (omp, OpenCode) keep their own model."
      >
        <ModelCombobox
          aria-label="Subagent model"
          className="max-w-64 max-md:w-full max-md:max-w-none"
          models={models.data ?? []}
          value={subagentModel.data?.id}
          none={SAME_AS_NEW}
          disabled={subagentModel.isPending || setSubagentModel.isPending}
          onChange={(v) => setSubagentModel.mutate(v)}
        />
      </Row>
      <Row stack label="theuth agent" description="The built-in root on the Agents page: its desk and the work it starts for tasks.">
        <ModelCombobox
          aria-label="theuth agent model"
          className="max-w-64 max-md:w-full max-md:max-w-none"
          models={models.data ?? []}
          value={theuthModel.data?.id}
          none={SAME_AS_NEW}
          disabled={theuthModel.isPending || setTheuthModel.isPending}
          onChange={(v) => setTheuthModel.mutate(v)}
        />
      </Row>
    </Section>
  )
}
