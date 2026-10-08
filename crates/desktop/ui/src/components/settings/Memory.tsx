import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Count, Row, Section, useAction } from '@/components/settings/shared'
import { Switch } from '@/components/ui/switch'
import { useCompactorLabel } from '@/lib/endless'
import { queryKeys, useApi } from '@/lib/rpc'
import type { EndlessConfig } from '@/rpc/endless-stubs'

/** `≈ 21 k tokens` for 64,000 bytes: content runs at ~3 bytes per token (137 §2). */
export const tokenEstimate = (bytes: number): string => `≈ ${Math.round(bytes / 3 / 1000)} k tokens`

/** A native range: `onChange` follows the pointer (the label beside it previews), `onCommit` fires on release so a drag is one write. */
function Range({ value, min, max, step, onChange, onCommit, disabled, 'aria-label': ariaLabel }: { value: number; min: number; max: number; step: number; onChange(v: number): void; onCommit(v: number): void; disabled: boolean; 'aria-label': string }) {
  return (
    <input
      type="range"
      aria-label={ariaLabel}
      className="w-48 accent-primary disabled:opacity-50"
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(Number(e.target.value))}
      onPointerUp={(e) => onCommit(Number(e.currentTarget.value))}
      onKeyUp={(e) => onCommit(Number(e.currentTarget.value))}
    />
  )
}

/** Settings › Memory (137 §11.3): the endless defaults, view size, slack, settle wait and the compactor model (`agent.endless`). */
export function Memory() {
  const api = useApi()
  const settings = useQuery({ queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() })
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const save = useAction((next: EndlessConfig) => api.endless_settings_set(next), { keys: [queryKeys.endlessSettings] })
  const s = settings.data
  const busy = !s || save.isPending
  const patch = (p: Partial<EndlessConfig>) => s && save.mutate({ ...s, ...p })
  // Live preview while the slider moves: the label and the token estimate follow the thumb before the write.
  const [viewKb, setViewKb] = useState<number | null>(null)
  const [slackPct, setSlackPct] = useState<number | null>(null)
  const kb = viewKb ?? (s ? s.view_bytes / 1000 : 128)
  const pct = slackPct ?? (s ? s.slack * 100 : 12.5)
  // R1-D3: `null` resolves to the title model when one is set; the picker's first row names what that is right now.
  const resolved = useCompactorLabel(null)

  return (
    <>
      <Section title="Defaults">
        <Row label="New chats start endless" description="The ∞ switch on the New chat view starts on; each draft can still turn it off.">
          <Switch aria-label="New chats start endless" checked={s?.default_chat ?? false} disabled={busy} onCheckedChange={(v) => patch({ default_chat: v })} />
        </Row>
        <Row label="Desks start endless" description="Desks whose agents file has no memory: field open their session in endless mode; a desk already running switches at its next idle moment and backfills its history.">
          <Switch aria-label="Desks start endless" checked={s?.default_desk ?? false} disabled={busy} onCheckedChange={(v) => patch({ default_desk: v })} />
        </Row>
        <Row label="Meetings start endless" description="The ∞ switch on the meeting start form starts on: the meeting keeps a compressed memory of the whole call behind the live transcript.">
          <Switch aria-label="Meetings start endless" checked={s?.default_meeting ?? false} disabled={busy} onCheckedChange={(v) => patch({ default_meeting: v })} />
        </Row>
      </Section>
      <Section title="View">
        <Row
          label={
            <>
              View size · <span className="tabular-nums">{kb} KB</span>
            </>
          }
          description={
            settings.isError
              ? `Could not read the memory settings: ${settings.error.message}`
              : `${tokenEstimate(kb * 1000)} of the whole chat the model reads every turn; the distant past gets coarser to fit. 16–256 KB.`
          }
        >
          <Range
            aria-label="View size in KB"
            value={kb}
            min={16}
            max={256}
            step={8}
            disabled={busy}
            onChange={setViewKb}
            onCommit={(v) => {
              setViewKb(null)
              patch({ view_bytes: v * 1000 })
            }}
          />
        </Row>
        <Row
          label={
            <>
              Slack · <span className="tabular-nums">{pct} %</span>
            </>
          }
          description="The view folds only once it outgrows the size by this much, then merges down to as far below it: fewer, larger rewrites and more of the view cached turn to turn."
        >
          <Range
            aria-label="Slack in percent"
            value={pct}
            min={0}
            max={50}
            step={2.5}
            disabled={busy}
            onChange={setSlackPct}
            onCommit={(v) => {
              setSlackPct(null)
              patch({ slack: v / 100 })
            }}
          />
        </Row>
        <Row label="Settle seconds" description="How long a turn waits for the summariser to finish the previous turn's lines before it starts; past it, short unbuilt messages go in verbatim. 5–180.">
          <Count value={s?.settle_secs} min={5} max={180} action={{ mutate: (n) => patch({ settle_secs: n }), isPending: save.isPending }} />
        </Row>
      </Section>
      <Section title="Compactor">
        <Row label="Compactor model" description="Writes the one-line summaries in the background. Unset, it follows the title model (your cheap side-call choice) when one is set, else the session's own model.">
          <ModelCombobox
            aria-label="Compactor model"
            className="max-w-64"
            models={models.data ?? []}
            value={s?.model}
            none={{ label: resolved }}
            disabled={busy}
            onChange={(id) => patch({ model: id })}
          />
        </Row>
        <Row label="Guards" description="Every summary line is checked before it is kept: no copied text, nothing the message does not say, no status words, one line in the format. Failures retry with the correction named.">
          <span className="text-sm text-fg-secondary">copy · grounding · status · format — always on</span>
        </Row>
      </Section>
    </>
  )
}
