import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { ChevronRight } from 'lucide-react'
import { Fragment, useEffect, useState } from 'react'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { Count, Range, Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { SummaryTestCall } from '@/components/settings/SummaryTestCall'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { ENDLESS_DEFAULTS, autoProfile, scaledMarks, tailPairError, tokenEstimate, useCompactorLabel } from '@/lib/endless'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CompactStyle, CompactorProfile, EndlessConfig, TailMode } from "@/rpc/bindings/method.agent.output"
import { useConnection } from '@/store/connection'

/** The numeric keys of `ENDLESS_DEFAULTS`: the Advanced rows' fields. */
type Key = { [K in keyof typeof ENDLESS_DEFAULTS]: (typeof ENDLESS_DEFAULTS)[K] extends number ? K : never }[keyof typeof ENDLESS_DEFAULTS]

const PROFILES: [CompactorProfile, string][] = [
  ['auto', 'Automatic'],
  ['standard', 'Standard'],
  ['small', 'Small'],
]

const COMPACT: [CompactStyle, string][] = [
  ['summary', 'Summary'],
  ['cut', 'Cut'],
  ['snap', 'Snap'],
]

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
      ['compactor_ctx_bytes', 'Summary context', 'Chat history sent with each summary call, in bytes, any model. Standard profile only. 8000–256000, default 48000.', 8_000, 256_000, 1],
    ],
  ],
  [
    'Speed & spend',
    [
      ['batch_units', 'Lines per summary call', 'Waiting lines summarized together in one call. Standard profile only. 1–64, default 16.', 1, 64, 1],
      ['jobs', 'Summaries at once (cloud)', 'Parallel summary calls per chat. Standard profile only. 1–32, default 8.', 1, 32, 1],
      ['max_usd_per_call', 'Spend per call (USD)', 'The most one summary call may cost. 0.001–10, default 0.10.', 0.001, 10, 0.001],
      ['max_usd_per_session_day', 'Daily spend per chat (USD)', "Settling pauses for the rest of the UTC day once a chat's summaries cost this much; 0 = no cap. 0–1000, default 5.", 0, 1000, 0.01],
    ],
  ],
]

/**
 * Settings › Agent › Context (tab id `memory`; 137 §11.3, 201 §12.2): the endless defaults, view size, slack, the verbatim tail, the compactor, the ledger and recall (`agent.endless`),
 * and the compact style of chats that are not endless (188 A9, from the old Agent loop tab).
 * `focus`: `model` focuses the compactor model (the Memory panel's "Change model", 137F); `spend` opens Advanced on the
 * daily spend cap (the paused-by-spend row's link, 137G).
 */
export function Memory({ focus = null }: { focus?: string | null }) {
  const canQuery1 = useQuerySupport(["endless_settings_get"])
  const canQuery1_2 = useQuerySupport(["list_models"])
  const canQuery1_3 = useQuerySupport(["title_model"])
  const api = useApi()
  const info = useConnection((s) => s.info)
  const settings = useQuery({ enabled: canQuery1, queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() })
  const models = useQuery({ enabled: canQuery1_2, queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const save = useAction((next: EndlessConfig) => api.endless_settings_set(next), { keys: [queryKeys.endlessSettings] })
  const s = settings.data
  const busy = !s || save.isPending
  const patch = (p: Partial<EndlessConfig>) => s && save.mutate({ ...s, ...p })
  const setCompact = useAction((style: CompactStyle) => api.set_compact_style(style).then(() => refreshInfo(api)))
  // Live preview while the slider moves: the label and the token estimate follow the thumb before the write.
  const [viewKb, setViewKb] = useState<number | null>(null)
  const [slackPct, setSlackPct] = useState<number | null>(null)
  const kb = viewKb ?? (s ? s.view_bytes / 1000 : 128)
  const pct = slackPct ?? (s ? s.slack * 100 : 12.5)
  const [mergePct, setMergePct] = useState<number | null>(null)
  const merge = mergePct ?? Math.round((s?.merge_at ?? ENDLESS_DEFAULTS.merge_at) * 100)
  // 201 §4.2: the tail pair in the UI's unit (KB in fixed mode, percent in scaled), previewed while a thumb moves.
  const mode: TailMode = s?.tail_mode ?? ENDLESS_DEFAULTS.tail_mode
  const fixed = mode === 'fixed'
  const toUi = (v: number) => (fixed ? v / 1000 : Math.round(v * 100))
  const toWire = (v: number) => (fixed ? v * 1000 : v / 100)
  const [tailLow, setTailLow] = useState<number | null>(null)
  const [tailHigh, setTailHigh] = useState<number | null>(null)
  const [tailRefused, setTailRefused] = useState<string | null>(null)
  const lowUi = tailLow ?? toUi(fixed ? (s?.tail_low_bytes ?? ENDLESS_DEFAULTS.tail_low_bytes) : (s?.tail_low_share ?? ENDLESS_DEFAULTS.tail_low_share))
  const highUi = tailHigh ?? toUi(fixed ? (s?.tail_high_bytes ?? ENDLESS_DEFAULTS.tail_high_bytes) : (s?.tail_high_share ?? ENDLESS_DEFAULTS.tail_high_share))
  // The high mark must stay above the low one: checked in place while dragging, and again before the write.
  const tailLive = tailLow !== null || tailHigh !== null ? tailPairError(mode, toWire(lowUi), toWire(highUi)) : null
  const commitTail = (low: number, high: number) => {
    setTailLow(null)
    setTailHigh(null)
    const err = tailPairError(mode, toWire(low), toWire(high))
    setTailRefused(err)
    if (err) return
    patch(fixed ? { tail_low_bytes: toWire(low), tail_high_bytes: toWire(high) } : { tail_low_share: toWire(low), tail_high_share: toWire(high) })
  }
  // The engine's refusal (`validate_201` names both values) when it still refuses a pair.
  const tailServer = save.error && /tail_/.test(save.error.message) ? save.error.message : null
  const tailProblem = tailLive ?? tailRefused ?? tailServer
  // Scaled marks resolved for the default model, estimated from its window.
  const defaultModel = models.data?.find((m) => m.id === info?.default_model)
  const resolvedTail = fixed ? null : scaledMarks(toWire(lowUi), toWire(highUi), defaultModel?.context ?? 0)
  const onModel = resolvedTail ? ` on ${defaultModel?.name ?? info?.default_model}` : ''
  // 177 U5: the model a summary call would use now — the compactor model, else the title model; else each chat's own.
  const title = useQuery({ queryKey: queryKeys.titleModel, queryFn: () => api.title_model(), enabled: canQuery1_3 && (!!s && !s.model) })
  const testModel = s?.model ?? title.data?.id
  // R1-D3: `null` resolves to the title model when one is set; the picker's first row names what that is right now.
  const resolved = useCompactorLabel(null)
  // 194: the picked compactor model's effort chips (none while unset: the fallback chain has no effort).
  const compactor = models.data?.find((m) => m.id === s?.model)
  // 201 §7.1: what Automatic resolves to for the model a summary call would use now.
  const profile = s?.profile ?? ENDLESS_DEFAULTS.profile
  const resolvedProfile = autoProfile(models.data?.find((m) => m.id === testModel))
  // The control is disabled until the settings load, and a disabled control takes no focus.
  useEffect(() => {
    if (busy) return
    if (focus === 'model') document.getElementById('compactor-model')?.focus()
    if (focus === 'spend') document.querySelector<HTMLInputElement>('input[aria-label="Daily spend per chat (USD)"]')?.focus()
  }, [focus, busy])

  return (
    <>
      <Section title="Defaults">
        <Row label="New chats use fractal context" description="The ∞ switch on the New chat view starts on; each draft can still turn it off.">
          <Switch aria-label="New chats use fractal context" checked={s?.default_chat ?? false} disabled={busy} onCheckedChange={(v) => patch({ default_chat: v })} />
        </Row>
        <Row label="Desks use fractal context" description="Desks whose agents file does not pick a mode open their session with fractal context; a desk already running switches at its next idle moment and backfills its history.">
          <Switch aria-label="Desks use fractal context" checked={s?.default_desk ?? false} disabled={busy} onCheckedChange={(v) => patch({ default_desk: v })} />
        </Row>
        <Row label="Meetings use fractal context" description="The ∞ switch on the meeting start form starts on: the meeting keeps a compressed view of the whole call behind the live transcript.">
          <Switch aria-label="Meetings use fractal context" checked={s?.default_meeting ?? false} disabled={busy} onCheckedChange={(v) => patch({ default_meeting: v })} />
        </Row>
        <Row label="Default compact style" description="How a compaction builds the new context head.">
          <Select value={info?.compact_style} disabled={!info || setCompact.isPending} onValueChange={(v) => setCompact.mutate(v as CompactStyle)}>
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
      </Section>
      <Section title="View">
        <Row
          stack
          label={
            <>
              View size · <span className="tabular-nums">{kb} KB</span>
            </>
          }
          description={
            settings.isError
              ? `Could not read the context settings: ${settings.error.message}`
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
          stack
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
        <Row
          stack
          label={
            <>
              Start merging at · <span className="tabular-nums">{merge} %</span>
            </>
          }
          description="Summary lines start merging into coarser ones once the view reaches this share of its size, so the merges are ready before they are needed. 50–100 %, default 75 %."
        >
          <Range
            aria-label="Start merging at in percent"
            value={merge}
            min={50}
            max={100}
            step={5}
            disabled={busy}
            onChange={setMergePct}
            onCommit={(v) => {
              setMergePct(null)
              patch({ merge_at: v / 100 })
            }}
          />
        </Row>
      </Section>
      <Section title="Tail">
        <Row label="Tail sizing" description="How much of the newest chat the model reads word for word. Fixed size keeps the same bytes on every model; Scaled to the model takes a share of each model's window. Default Fixed size.">
          <Select
            name="tail_mode"
            value={mode}
            disabled={busy}
            onValueChange={(v) => {
              setTailRefused(null)
              patch({ tail_mode: v as TailMode })
            }}
          >
            <SelectTrigger className="w-48" aria-label="Tail sizing">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="fixed">Fixed size</SelectItem>
              <SelectItem value="scaled">Scaled to the model</SelectItem>
            </SelectContent>
          </Select>
        </Row>
        <Row
          stack
          label={
            <>
              Kept after a trim · <span className="tabular-nums">{fixed ? `${lowUi} KB` : `${lowUi} %`}</span>
              {resolvedTail && <span className="text-fg-tertiary tabular-nums"> · ≈ {Math.round(resolvedTail.low / 1000)} KB</span>}
            </>
          }
          description={
            fixed
              ? `${tokenEstimate(lowUi * 1000)} of the newest messages stay word for word after a trim. 8–256 KB, default 24 KB.`
              : `The share of the model's turn allowance that stays word for word after a trim${onModel}${resolvedTail ? '' : '; resolved per chat from its model'}. 2–40 %, default 10 %.`
          }
        >
          <Range
            aria-label={fixed ? 'Kept after a trim in KB' : 'Kept after a trim in percent'}
            value={lowUi}
            min={fixed ? 8 : 2}
            max={fixed ? 256 : 40}
            step={fixed ? 4 : 1}
            disabled={busy}
            onChange={setTailLow}
            onCommit={(v) => commitTail(v, highUi)}
          />
        </Row>
        <Row
          stack
          label={
            <>
              Trim at · <span className="tabular-nums">{fixed ? `${highUi} KB` : `${highUi} %`}</span>
              {resolvedTail && <span className="text-fg-tertiary tabular-nums"> · ≈ {Math.round(resolvedTail.high / 1000)} KB</span>}
            </>
          }
          description={
            fixed
              ? `${tokenEstimate(highUi * 1000)}: once the word-for-word part grows past this, its oldest turns move into the summaries in one cache rewrite. Above Kept after a trim. 16–512 KB, default 64 KB.`
              : `The share at which a trim is due${onModel}; above Kept after a trim. 5–60 %, default 25 %.`
          }
        >
          <Range
            aria-label={fixed ? 'Trim at in KB' : 'Trim at in percent'}
            value={highUi}
            min={fixed ? 16 : 5}
            max={fixed ? 512 : 60}
            step={fixed ? 8 : 1}
            disabled={busy}
            onChange={setTailHigh}
            onCommit={(v) => commitTail(lowUi, v)}
          />
        </Row>
        {tailProblem && (
          <p role="alert" className="pt-2 text-xs text-danger">
            {tailProblem}
          </p>
        )}
        <Row label="Trim early when idle" description="After this many idle seconds the provider's cache has expired anyway, so the next turn trims down to Kept after a trim at no extra cost. Empty = automatic: the provider's cache lifetime (300 s on Anthropic). 0–86400 seconds.">
          <Count aria-label="Trim early when idle in seconds" value={s ? s.cold_after_secs || undefined : undefined} placeholder={s ? 'automatic' : undefined} blank={0} min={0} max={86_400} action={{ mutate: (n) => patch({ cold_after_secs: n }), isPending: save.isPending }} />
        </Row>
      </Section>
      <Section title="Compactor">
        <Row
          stack
          label="Compactor model"
          description="Writes the one-line summaries in the background. Unset, it follows the title model (your cheap side-call choice) when one is set, else the session's own model. A change applies to the next summary call, and calls still running on the previous model are cancelled; no restart. Local models summarise one call at a time with a longer timeout: slower, but they finish. Its effort chips set how hard it thinks; Default keeps thinking off."
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
            effort={compactor && compactor.efforts.length > 0 ? { levels: compactor.efforts, current: s?.effort, onSelect: (effort) => patch({ effort }) } : undefined}
          />
        </Row>
        <Row
          wide
          label="Test summary call"
          description={testModel ? `Sends one summary of a fixed sample to ${testModel}, as the summariser would, and shows what came back.` : "Unset with no title model, each chat summarises with its own model: pick a compactor model to test it."}
        >
          <SummaryTestCall model={testModel} />
        </Row>
        <Row
          label="Profile"
          description={
            <>
              How the summariser works. Standard batches lines with a long chat context; Small sends one message per call with a short prompt, for tiny or local models. Automatic picks Small for a local model or one that reads 32 k tokens or less. Default Automatic.
              {profile === 'auto' && <span className="block pt-1 text-fg-tertiary">{resolvedProfile ? `Automatic · ${resolvedProfile.profile}, because ${resolvedProfile.reason}` : 'Automatic · decided per chat from its summary model'}</span>}
            </>
          }
        >
          <Select name="profile" value={profile} disabled={busy} onValueChange={(v) => patch({ profile: v as CompactorProfile })}>
            <SelectTrigger className="w-40" aria-label="Compactor profile">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PROFILES.map(([id, label]) => (
                <SelectItem key={id} value={id}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label="Describe images" description="When an image or PDF leaves the word-for-word part, the summariser writes a one-line description of it; zoom brings the original back. Default on.">
          <Switch aria-label="Describe images" checked={(s?.describe_images ?? ENDLESS_DEFAULTS.describe_images) !== 'off'} disabled={busy} onCheckedChange={(v) => patch({ describe_images: v ? 'auto' : 'off' })} />
        </Row>
        <Row stack label="Guards" description="Every summary line is checked before it is kept: no copied text, nothing the message does not say, no status words, one line in the format. Failures retry with the correction named.">
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
      <Section title="Ledger">
        <Row label="Keep a state ledger" description="A short record of what is true now (goals, decisions, constraints, open tasks, files touched) that the model keeps current through a ledger tool and reads every turn. Default on.">
          <Switch aria-label="Keep a state ledger" checked={s?.ledger ?? ENDLESS_DEFAULTS.ledger} disabled={busy} onCheckedChange={(v) => patch({ ledger: v })} />
        </Row>
        <Row label="Ledger size" description="How much the ledger may take in each turn, in KB. 1–16, default 4.">
          <Count aria-label="Ledger size in KB" value={s ? (s.ledger_bytes ?? ENDLESS_DEFAULTS.ledger_bytes) / 1000 : undefined} min={1} max={16} action={{ mutate: (n) => patch({ ledger_bytes: n * 1000 }), isPending: save.isPending }} />
        </Row>
        <Row label="Show summary-model suggestions" description="What the summariser notices (a decision made, a task opened) appears in the ledger as an unconfirmed suggestion for the model to confirm or drop. Needs the ledger. Default on.">
          <Switch aria-label="Show summary-model suggestions" checked={s?.ledger_suggestions ?? ENDLESS_DEFAULTS.ledger_suggestions} disabled={busy || !(s?.ledger ?? ENDLESS_DEFAULTS.ledger)} onCheckedChange={(v) => patch({ ledger_suggestions: v })} />
        </Row>
      </Section>
      <Section title="Recall">
        <Row label="Prefetch size" description="Recall hits for the new message, in bytes, added after the newest messages so the model sees relevant older detail without asking; 0 turns it off. 0–8000, default 2000.">
          <Count aria-label="Prefetch size in bytes" value={s?.prefetch_bytes} placeholder={s && ENDLESS_DEFAULTS.prefetch_bytes} min={0} max={8_000} action={{ mutate: (n) => patch({ prefetch_bytes: n }), isPending: save.isPending }} />
        </Row>
        <Row label="Keep zoomed lines unmerged" description="A summary line the model zoomed into keeps its detail through the next trim instead of merging into a coarser one. Default on.">
          <Switch aria-label="Keep zoomed lines unmerged" checked={s?.age_by_use ?? ENDLESS_DEFAULTS.age_by_use} disabled={busy} onCheckedChange={(v) => patch({ age_by_use: v })} />
        </Row>
      </Section>
    </>
  )
}
