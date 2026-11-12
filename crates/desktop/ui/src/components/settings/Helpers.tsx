import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { useModels } from '@/components/agents/queries'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Row, Section, useAction } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'

/** 188 A5: Settings › Models › Helpers — the cheap side-call models: chat titles, the vision fallback, the oracle's escalation. */
export function Helpers() {
  const canQuery1 = useQuerySupport(["title_model"])
  const canQuery1_2 = useQuerySupport(["title_effort"])
  const canQuery1_3 = useQuerySupport(["vision_model"])
  const canQuery1_4 = useQuerySupport(["vision_effort"])
  const canQuery1_5 = useQuerySupport(["escalate_model"])
  const canQuery1_6 = useQuerySupport(["escalate_effort"])
  const api = useApi()
  const models = useModels()
  const titleModel = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.titleModel,
    queryFn: () => api.title_model(),
  })
  const titleEffort = useQuery({ enabled: canQuery1_2, queryKey: queryKeys.titleEffort, queryFn: () => api.title_effort() })
  const visionModel = useQuery({ enabled: canQuery1_3,
    queryKey: queryKeys.visionModel,
    queryFn: () => api.vision_model(),
  })
  const visionEffort = useQuery({ enabled: canQuery1_4, queryKey: queryKeys.visionEffort, queryFn: () => api.vision_effort() })

  // 194 D4c/D4d: a model change may drop the stored effort (Off clears it), so its key refetches too.
  const setTitleModel = useAction((id: string) => api.set_title_model(id === 'off' ? null : id), {
    keys: [queryKeys.titleModel, queryKeys.titleEffort, queryKeys.escalateModel],
  })
  const setTitleEffort = useAction((level: string | null) => api.set_title_effort(level), { keys: [queryKeys.titleEffort] })
  const setVisionModel = useAction((id: string) => api.set_vision_model(id === 'off' ? null : id), {
    keys: [queryKeys.visionModel, queryKeys.visionEffort],
  })
  const setVisionEffort = useAction((level: string | null) => api.set_vision_effort(level), { keys: [queryKeys.visionEffort] })
  const escalateModel = useQuery({ enabled: canQuery1_5,
    queryKey: queryKeys.escalateModel,
    queryFn: () => api.escalate_model(),
  })
  const escalateEffort = useQuery({ enabled: canQuery1_6, queryKey: queryKeys.escalateEffort, queryFn: () => api.escalate_effort() })
  // 57 §0.9: the effective model follows the title model, so both keys refresh.
  const setEscalateModel = useAction((id: string) => api.set_escalate_model(id === 'off' ? null : id), {
    keys: [queryKeys.escalateModel, queryKeys.escalateEffort],
  })
  const setEscalateEffort = useAction((level: string | null) => api.set_escalate_effort(level), { keys: [queryKeys.escalateEffort] })

  // 52 §3.2: only models whose backend can actually receive images (`ModelInfo.image` is effective support).
  const visionModels = models.data?.filter((m) => m.image) ?? []
  const vision = visionModel.data
  // 194 D5: each row's chips are the levels of the model it shows as selected.
  const title = models.data?.find((m) => m.id === titleModel.data?.id)
  const visionInfo = visionModels.find((m) => m.id === vision?.id)
  const escalate = models.data?.find((m) => m.id === escalateModel.data?.model?.id)
  // A saved selection whose provider went away stays visible as unavailable, never as Off.
  const unavailable =
    vision && models.isSuccess && !visionModels.some((m) => m.id === vision.id) ? vision.id : null

  return (
    <Section title="Helper models">
      <Row stack label="Title model" description="Names new chats after the first exchange.">
        <ModelCombobox
          aria-label="Title model"
          className="max-w-64 max-md:w-full max-md:max-w-none"
          models={models.data ?? []}
          value={titleModel.data?.id}
          none={{ label: 'Off' }}
          effort={title && title.efforts.length > 0 ? { levels: title.efforts, current: titleEffort.data, onSelect: (level) => setTitleEffort.mutate(level) } : undefined}
          disabled={titleModel.isPending || setTitleModel.isPending || setTitleEffort.isPending}
          onChange={(v) => setTitleModel.mutate(v ?? 'off')}
        />
      </Row>
      <Row
        stack
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
          className="max-w-64 max-md:w-full max-md:max-w-none"
          models={visionModels}
          value={vision?.id}
          none={{ label: 'Off' }}
          effort={visionInfo && visionInfo.efforts.length > 0 ? { levels: visionInfo.efforts, current: visionEffort.data, onSelect: (level) => setVisionEffort.mutate(level) } : undefined}
          disabled={!visionModel.isSuccess || setVisionModel.isPending || setVisionEffort.isPending}
          onChange={(v) => setVisionModel.mutate(v ?? 'off')}
        />
      </Row>
      <Row
        stack
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
          className="max-w-64 max-md:w-full max-md:max-w-none"
          models={models.data ?? []}
          value={escalateModel.data?.model?.id}
          none={{ label: escalateModel.data?.effective ? `Off (inherits title model ${escalateModel.data.effective.id})` : 'Off (inherits title model)' }}
          effort={escalate && escalate.efforts.length > 0 ? { levels: escalate.efforts, current: escalateEffort.data, onSelect: (level) => setEscalateEffort.mutate(level) } : undefined}
          disabled={!escalateModel.isSuccess || setEscalateModel.isPending || setEscalateEffort.isPending}
          onChange={(v) => setEscalateModel.mutate(v ?? 'off')}
        />
      </Row>
    </Section>
  )
}
