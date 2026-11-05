import { useQuery } from '@tanstack/react-query'
import { ChevronRight } from 'lucide-react'
import { Fragment, useEffect, useState } from 'react'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Count, Row, Section, useAction } from '@/components/settings/shared'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Switch } from '@/components/ui/switch'
import { ENDLESS_DEFAULTS, type EndlessConfigX, tokenEstimate, useCompactorLabel } from '@/lib/endless'
import { queryKeys, useApi } from '@/lib/rpc'
import type { EndlessConfig } from '@/rpc/bindings/EndlessConfig'

type Key = keyof typeof ENDLESS_DEFAULTS

/** The Advanced rows by group: key, label, help, the engine's validated range, and the step (1 = whole numbers). The engine default is the placeholder while the key is absent. */
export const ADVANCED: readonly (readonly [string | null, readonly (readonly [Key, string, string, number, number, number])[]])[] = [
  [
    null,
    [
      ['attempt_secs', 'Attempt seconds', 'Cloud attempt limit: how long one summary call to a cloud model may run before it is cancelled and retried. 10–3600, default 120.', 10, 3600, 1],
      ['local_attempt_secs', 'Local attempt seconds', 'Local attempt limit: the same for a local model, which is slower to load and to answer. 30–7200, default 600.', 30, 7200, 1],
      ['local_ctx_bytes', 'Local context bytes', 'Local prompt history size: how much of the chat a local model reads per summary call. 4000–256000, default 24000.', 4_000, 256_000, 1],
      ['local_jobs', 'Local jobs', 'Local summaries at once: calls a local model runs in parallel. 1–16, default 1.', 1, 16, 1],
    ],
  ],
  [
    'Request size',
    [
      ['raw_bytes', 'Unsummarized lines in full', 'Newest waiting lines sent verbatim, in bytes; older ones fold to one line. 4000–128000, default 32000.', 4_000, 128_000, 1],
      ['compactor_ctx_bytes', 'Summary context', 'Chat history sent with each summary call, in bytes, any model. 8000–256000, default 48000.', 8_000, 256_000, 1],
    ],
  ],
  [
    'Speed & spend',
    [
      ['batch_units', 'Lines per summary call', 'Waiting lines summarized together in one call. 1–64, default 16.', 1, 64, 1],
      ['jobs', 'Summaries at once (cloud)', 'Parallel summary calls per chat. 1–32, default 8.', 1, 32, 1],
      ['max_usd_per_call', 'Spend per call (USD)', 'The most one summary call may cost. 0.001–10, default 0.10.', 0.001, 10, 0.001],
      ['max_usd_per_session_day', 'Daily spend per chat (USD)', "Settling pauses for the rest of the UTC day once a chat's summaries cost this much; 0 = no cap. 0–1000, default 5.", 0, 1000, 0.01],
    ],
  ],
]

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

/**
 * Settings › Memory (137 §11.3): the endless defaults, view size, slack, settle wait and the compactor model (`agent.endless`).
 * `focus`: `model` focuses the compactor model (the Memory panel's "Change model", 137F); `spend` opens Advanced on the
 * daily spend cap (the paused-by-spend row's link, 137G).
 */
export function Memory({ focus = null }: { focus?: string | null }) {
  const api = useApi()
  const settings = useQuery({ queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() })
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const save = useAction((next: EndlessConfig) => api.endless_settings_set(next), { keys: [queryKeys.endlessSettings] })
  const s = settings.data as EndlessConfigX | undefined
  const busy = !s || save.isPending
  const patch = (p: Partial<EndlessConfigX>) => s && save.mutate({ ...s, ...p })
  // Live preview while the slider moves: the label and the token estimate follow the thumb before the write.
  const [viewKb, setViewKb] = useState<number | null>(null)
  const [slackPct, setSlackPct] = useState<number | null>(null)
  const kb = viewKb ?? (s ? s.view_bytes / 1000 : 128)
  const pct = slackPct ?? (s ? s.slack * 100 : 12.5)
  // R1-D3: `null` resolves to the title model when one is set; the picker's first row names what that is right now.
  const resolved = useCompactorLabel(null)
  // The control is disabled until the settings load, and a disabled control takes no focus.
  useEffect(() => {
    if (busy) return
    if (focus === 'model') document.getElementById('compactor-model')?.focus()
    if (focus === 'spend') document.querySelector<HTMLInputElement>('input[aria-label="Daily spend per chat (USD)"]')?.focus()
  }, [focus, busy])

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
        <Row
          stack
          label="Compactor model"
          description="Writes the one-line summaries in the background. Unset, it follows the title model (your cheap side-call choice) when one is set, else the session's own model. A change applies to the next summary call, and calls still running on the previous model are cancelled; no restart. Local models summarise one call at a time with a longer timeout: slower, but they finish."
        >
          <ModelCombobox
            id="compactor-model"
            aria-label="Compactor model"
            className="max-w-64 max-md:w-full max-md:max-w-none"
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
        <Collapsible defaultOpen={focus === 'spend'}>
          <CollapsibleTrigger className="mt-3 flex items-center gap-1 text-[13px] text-fg-secondary [&[data-state=open]>svg]:rotate-90">
            <ChevronRight className="size-3.5 transition-transform" /> Advanced
          </CollapsibleTrigger>
          {/* Mounted while closed (hidden by `data-state`, as `forceMount` keeps Radix from hiding it), so the fields render statically. */}
          <CollapsibleContent forceMount className="pt-1 data-[state=closed]:hidden">
            {ADVANCED.map(([group, rows]) => (
              <Fragment key={group ?? ''}>
                {group && <h4 className="mt-4 text-[13px] font-semibold">{group}</h4>}
                {rows.map(([key, label, help, min, max, step]) => (
                  <Row key={key} label={label} description={help}>
                    <Count aria-label={label} value={s?.[key]} min={min} max={max} step={step < 1 ? step : undefined} placeholder={s && ENDLESS_DEFAULTS[key]} action={{ mutate: (n) => patch({ [key]: n }), isPending: save.isPending }} />
                  </Row>
                ))}
              </Fragment>
            ))}
          </CollapsibleContent>
        </Collapsible>
      </Section>
    </>
  )
}
