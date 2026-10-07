import { useQuery } from '@tanstack/react-query'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Row, Section, bounded, refreshInfo, useAction } from '@/components/settings/shared'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useNudges } from '@/lib/nudges'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import { useConnection } from '@/store/connection'

const COMPACT: [CompactStyle, string][] = [
  ['summary', 'Summary'],
  ['cut', 'Cut'],
  ['snap', 'Snap'],
]

/** A whole-number `Info` field edited on blur/Enter; values outside `[min, max]` are ignored. Shared with Settings › Advisor. */
export function Count({
  value,
  min,
  max,
  action,
}: {
  value: number | undefined
  min: number
  max?: number
  action: { mutate: (n: number) => void; isPending: boolean }
}) {
  return (
    <Input
      key={value}
      type="number"
      min={min}
      max={max}
      defaultValue={value}
      disabled={value === undefined || action.isPending}
      className="w-28 text-right"
      onBlur={(e) => {
        const n = bounded(e.currentTarget.value, min, max)
        if (n !== undefined && n !== value) action.mutate(n)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur()
      }}
    />
  )
}

/** Settings › Agent loop: how a turn runs — model calls, recovery, compaction, helper models. */
export function Loop() {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const showNudges = useNudges((s) => s.show)
  const models = useQuery({
    queryKey: queryKeys.models,
    queryFn: () => api.list_models(),
  })
  const titleModel = useQuery({
    queryKey: queryKeys.titleModel,
    queryFn: () => api.title_model(),
  })
  const subagentModel = useQuery({
    queryKey: queryKeys.subagentModel,
    queryFn: () => api.subagent_model(),
  })
  const visionModel = useQuery({
    queryKey: queryKeys.visionModel,
    queryFn: () => api.vision_model(),
  })

  const setTitleModel = useAction((id: string) => api.set_title_model(id === 'off' ? null : id), {
    keys: [queryKeys.titleModel, queryKeys.escalateModel],
  })
  const setSubagentModel = useAction((id: string) => api.set_subagent_model(id === 'off' ? null : id), {
    keys: [queryKeys.subagentModel],
  })
  const setVisionModel = useAction((id: string) => api.set_vision_model(id === 'off' ? null : id), {
    keys: [queryKeys.visionModel],
  })
  const escalateModel = useQuery({
    queryKey: queryKeys.escalateModel,
    queryFn: () => api.escalate_model(),
  })
  // 57 §0.9: the effective model follows the title model, so both keys refresh.
  const setEscalateModel = useAction((id: string) => api.set_escalate_model(id === 'off' ? null : id), {
    keys: [queryKeys.escalateModel],
  })
  const setCompact = useAction((style: CompactStyle) =>
    api.set_compact_style(style).then(() => refreshInfo(api)),
  )
  const setSubagentTimeout = useAction((seconds: number) =>
    api.set_subagent_timeout(seconds).then(() => refreshInfo(api)),
  )
  const setScheduleRunTimeout = useAction((seconds: number) =>
    api.set_schedule_run_timeout(seconds).then(() => refreshInfo(api)),
  )
  const setMaxToolRounds = useAction((rounds: number) =>
    api.set_max_tool_rounds(rounds).then(() => refreshInfo(api)),
  )
  const setStreamIdle = useAction((seconds: number) =>
    api.set_stream_idle(seconds).then(() => refreshInfo(api)),
  )
  const setMaxSalvages = useAction((count: number) =>
    api.set_max_salvages(count).then(() => refreshInfo(api)),
  )
  const setPtyIdle = useAction((seconds: number) =>
    api.set_pty_idle(seconds).then(() => refreshInfo(api)),
  )

  // 52 §3.2: only models whose backend can actually receive images (`ModelInfo.image` is effective support).
  const visionModels = models.data?.filter((m) => m.image) ?? []
  const vision = visionModel.data
  // A saved selection whose provider went away stays visible as unavailable, never as Off.
  const unavailable =
    vision && models.isSuccess && !visionModels.some((m) => m.id === vision.id) ? vision.id : null

  return (
    <>
      <Section title="Model calls">
        <Row
          label="Stream idle timeout"
          description="Seconds without any output from the model before the call is abandoned as a dead connection. Raise it if long tool arguments stall on your provider. New sessions only."
        >
          <Count value={info?.stream_idle_secs} min={1} action={setStreamIdle} />
        </Row>
        <Row
          label="Reply recoveries per turn"
          description="How many times per turn theuth picks up a reply cut off by a stalled stream or the output token limit before the turn stops with an error. 0 turns recovery off. New sessions only."
        >
          <Count value={info?.max_salvages} min={0} action={setMaxSalvages} />
        </Row>
        <Row
          label="Tool rounds per turn"
          description="Tool-call rounds a turn may run before the model is asked to reply with text. New sessions only."
        >
          <Count value={info?.max_tool_rounds} min={1} action={setMaxToolRounds} />
        </Row>
        <Row
          label="Subagent timeout"
          description="Maximum runtime for a newly spawned subagent turn, in seconds."
        >
          <Count value={info?.subagent_timeout_secs} min={1} action={setSubagentTimeout} />
        </Row>
        <Row
          label="Terminal idle wake"
          description="Seconds a PTY job's screen must stay unchanged before the model is woken with <job_idle> and the screen (a REPL at its prompt, a wizard waiting, a stalled build). Applies to later `start` calls; 0 turns it off, and `start … idle_secs` overrides per job."
        >
          <Count value={info?.pty_idle_secs} min={0} action={setPtyIdle} />
        </Row>
      </Section>
      <Section title="Routines">
        <Row
          label="Run time limit"
          description="Seconds a routine run may take before it is cancelled and its history row says timeout. A routine's own run_timeout: overrides it. Applies to runs fired from now on."
        >
          <Count value={info?.schedule_run_timeout_secs || undefined} min={30} action={setScheduleRunTimeout} />
        </Row>
      </Section>
      <Section title="Context">
        <Row
          label="Default compact style"
          description="How a compaction builds the new context head."
        >
          <Select
            value={info?.compact_style}
            disabled={!info || setCompact.isPending}
            onValueChange={(v) => setCompact.mutate(v as CompactStyle)}
          >
            <SelectTrigger className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {COMPACT.map(([id, label]) => (
                <SelectItem key={id} value={id}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Show theuth nudges"
          description="Show theuth's mid-turn notes to the model (continue, tool rounds, context pressure) in the chat. Display only."
        >
          <Switch checked={showNudges} onCheckedChange={useNudges.getState().setShow} />
        </Row>
      </Section>
      <Section title="Helper models">
        <Row label="Title model" description="Names new chats after the first exchange.">
          <ModelCombobox
            aria-label="Title model"
            className="max-w-64"
            models={models.data ?? []}
            value={titleModel.data?.id}
            none={{ label: 'Off' }}
            disabled={titleModel.isPending || setTitleModel.isPending}
            onChange={(v) => setTitleModel.mutate(v ?? 'off')}
          />
        </Row>
        <Row
          label="Subagent model"
          description="What spawn_subagent uses when the agent does not name a model. The agent is told your choice in its system prompt. Off = the session's model."
        >
          <ModelCombobox
            aria-label="Subagent model"
            className="max-w-64"
            models={models.data ?? []}
            value={subagentModel.data?.id}
            none={{ label: 'Off' }}
            disabled={subagentModel.isPending || setSubagentModel.isPending}
            onChange={(v) => setSubagentModel.mutate(v ?? 'off')}
          />
        </Row>
        <Row
          label="Vision fallback model"
          description={
            visionModel.isError
              ? `Could not read the vision fallback: ${visionModel.error.message}`
              : unavailable
                ? `${unavailable} is selected but not available on this node; pick another or Off.`
                : 'Used only when the main model cannot view images. It receives the image and a question written by the main model. Images may go to a different configured provider, and each new image batch costs a question call and a vision call; text redaction cannot remove secrets shown in pixels.'
          }
        >
          <ModelCombobox
            aria-label="Vision fallback model"
            className="max-w-64"
            models={visionModels}
            value={vision?.id}
            none={{ label: 'Off' }}
            disabled={!visionModel.isSuccess || setVisionModel.isPending}
            onChange={(v) => setVisionModel.mutate(v ?? 'off')}
          />
        </Row>
        <Row
          label="Escalation model"
          description={
            escalateModel.isError
              ? `Could not read the escalation model: ${escalateModel.error.message}`
              : `Tier 2 of the local decision oracle: answers a typed question the oracle abstained on, in one cheap no-tools call. Off = inherits the title model${
                  escalateModel.data?.model === null && escalateModel.data.effective
                    ? ` (${escalateModel.data.effective.id})`
                    : ", then the session's model"
                }.`
          }
        >
          <ModelCombobox
            aria-label="Escalation model"
            className="max-w-64"
            models={models.data ?? []}
            value={escalateModel.data?.model?.id}
            none={{ label: escalateModel.data?.effective ? `Off (inherits title model ${escalateModel.data.effective.id})` : 'Off (inherits title model)' }}
            disabled={!escalateModel.isSuccess || setEscalateModel.isPending}
            onChange={(v) => setEscalateModel.mutate(v ?? 'off')}
          />
        </Row>
      </Section>
    </>
  )
}
