import { useQuery } from '@tanstack/react-query'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Row, Section, useAction } from '@/components/settings/shared'
import { Switch } from '@/components/ui/switch'
import { queryKeys, useApi } from '@/lib/rpc'
import { useTalkSettings } from '@/lib/talk-settings'
import { useToolRuns } from '@/lib/tool-runs'
import { useConnection } from '@/store/connection'

/** `none` of the Subagents and theuth agent rows: both fall back to the new-sessions model (148 R2/R3). */
export const SAME_AS_NEW = { label: 'Same as new sessions' }

/**
 * Settings › Chat: the model defaults (148 — engine settings: new sessions, native subagents, the theuth agent),
 * then how the transcript displays and the talk view's cue (client-only preferences).
 */
export function Chat() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const collapse = useToolRuns((s) => s.collapse)
  const cue = useTalkSettings((s) => s.cue)
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
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
    <>
      <Section title="Models">
        <Row label="New sessions" description="The model every new chat starts on. Picking a model inside a chat changes only that chat.">
          <ModelCombobox
            aria-label="New sessions model"
            className="max-w-64"
            models={models.data ?? []}
            value={info?.default_model}
            effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: info?.default_effort, onSelect: (level) => setDefaultEffort.mutate(level) } : undefined}
            disabled={!info || setDefaultModel.isPending || setDefaultEffort.isPending}
            onChange={(v) => v && setDefaultModel.mutate(v)}
          />
        </Row>
        <Row
          label="Subagents"
          description="What theuth subagents start on unless you ask the agent for a specific model. External harnesses (omp, OpenCode) keep their own model."
        >
          <ModelCombobox
            aria-label="Subagent model"
            className="max-w-64"
            models={models.data ?? []}
            value={subagentModel.data?.id}
            none={SAME_AS_NEW}
            disabled={subagentModel.isPending || setSubagentModel.isPending}
            onChange={(v) => setSubagentModel.mutate(v)}
          />
        </Row>
        <Row label="theuth agent" description="The built-in root on the Agents page: its desk and the work it starts for tasks.">
          <ModelCombobox
            aria-label="theuth agent model"
            className="max-w-64"
            models={models.data ?? []}
            value={theuthModel.data?.id}
            none={SAME_AS_NEW}
            disabled={theuthModel.isPending || setTheuthModel.isPending}
            onChange={(v) => setTheuthModel.mutate(v)}
          />
        </Row>
      </Section>
      <Section title="Tool calls">
        <Row
          label="Collapse tool calls by default"
          description="Consecutive tool calls in a reply fold into one card with a live status line; click it to see every call. Off shows each call as its own card."
        >
          <Switch checked={collapse} onCheckedChange={useToolRuns.getState().setCollapse} />
        </Row>
      </Section>
      <Section title="Talk">
        <Row label="Turn chime" description="A soft two-tone chime when you stop talking and the model takes the turn.">
          <Switch checked={cue} onCheckedChange={useTalkSettings.getState().setCue} />
        </Row>
      </Section>
    </>
  )
}
