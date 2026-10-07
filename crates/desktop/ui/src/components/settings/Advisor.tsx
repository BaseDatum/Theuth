import { EFFORTS } from '@/components/agents/wizard'
import { Count, Row, Section, refreshInfo, useAction } from '@/components/settings/shared'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useApi } from '@/lib/rpc'
import type { SyncBacklog } from '@/rpc/bindings/SyncBacklog'
import { useConnection } from '@/store/connection'

/** `advisor.max_notes_per_update` bounds (125 §6.5). */
export const NOTES_RANGE = [1, 32] as const
/** `advisor.immune_turns` bounds (125 §6.5). */
export const IMMUNE_RANGE = [0, 10] as const

/** The Catch-up choices in display order: the `SyncBacklog` wire values, `off` labelled `Off`. */
export const BACKLOGS: readonly SyncBacklog[] = ['off', '1', '3', '5']
export const backlogLabel = (v: SyncBacklog): string => (v === 'off' ? 'Off' : v)
/** `s` as a `SyncBacklog`; `undefined` for anything the engine would refuse. */
export const asBacklog = (s: string): SyncBacklog | undefined => BACKLOGS.find((v) => v === s)

/** Radix `Select` refuses `''`: the Default row's value. */
const DEFAULT = '__default'

/** Settings › Advisor (125 §8.4): the defaults a new advisor starts with and the loop's tuning keys. No enable row — a session's advisor is picked in its header. */
export function Advisor() {
  const api = useApi()
  const advisor = useConnection((s) => s.info?.advisor)
  const setEffort = useAction((effort: string | null) => api.set_advisor_default_effort(effort).then(() => refreshInfo(api)))
  const setNotes = useAction((n: number) => api.set_advisor_notes_per_update(n).then(() => refreshInfo(api)))
  const setImmune = useAction((n: number) => api.set_advisor_immune_turns(n).then(() => refreshInfo(api)))
  const setBacklog = useAction((v: SyncBacklog) => api.set_advisor_sync_backlog(v).then(() => refreshInfo(api)))
  return (
    <Section title="Advisor">
      <Row label="Default effort" description="Thinking effort the advisor starts with when you pick a model.">
        <Select value={advisor?.effort ?? DEFAULT} disabled={!advisor || setEffort.isPending} onValueChange={(v) => setEffort.mutate(v === DEFAULT ? null : v)}>
          <SelectTrigger className="w-40" aria-label="Default effort">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={DEFAULT}>Default</SelectItem>
            {EFFORTS.map((e) => (
              <SelectItem key={e} value={e}>
                {e}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Notes per update" description="Non-blocker notes the advisor may send per update (1–32).">
        <Count value={advisor?.max_notes_per_update} min={NOTES_RANGE[0]} max={NOTES_RANGE[1]} action={setNotes} />
      </Row>
      <Row label="Immune turns" description="Steps after a delivered concern during which further concerns are asides (0 = off).">
        <Count value={advisor?.immune_turns} min={IMMUNE_RANGE[0]} max={IMMUNE_RANGE[1]} action={setImmune} />
      </Row>
      <Row label="Catch-up" description="Wait up to 30 s at the end of a turn while the advisor is this many updates behind.">
        <Select
          value={advisor?.sync_backlog}
          disabled={!advisor || setBacklog.isPending}
          onValueChange={(v) => {
            const b = asBacklog(v)
            if (b) setBacklog.mutate(b)
          }}
        >
          <SelectTrigger className="w-40" aria-label="Catch-up">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {BACKLOGS.map((v) => (
              <SelectItem key={v} value={v}>
                {backlogLabel(v)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
    </Section>
  )
}
