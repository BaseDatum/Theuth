// 156 D12/D13: the pure parts of transcript multi-selection and segment enrollment. The engine's rule for the
// voiceprint is "the word spans of the chosen segments, one track, at least 1 s"; `speechMs` and `enrollBlock`
// mirror it so the UI disables with the reason instead of round-tripping for the refusal.

import type { Enrolled } from '@/rpc/bindings/Enrolled'
import type { EnrollTarget } from '@/rpc/bindings/EnrollTarget'
import type { Segment } from '@/rpc/bindings/Segment'

/** The engine refuses less than this much speech. */
export const MIN_SPEECH_MS = 1000

/**
 * The next selection after clicking `id`: a plain click toggles it; a shift-click with an `anchor` adds every
 * id between the anchor and `id` in `order` (inclusive). Without an anchor, shift behaves as a plain click.
 */
export function selectClick(selected: ReadonlySet<number>, order: readonly number[], anchor: number | null, id: number, shift: boolean): Set<number> {
  const next = new Set(selected)
  const a = anchor === null ? -1 : order.indexOf(anchor)
  const b = order.indexOf(id)
  if (shift && a >= 0 && b >= 0) {
    for (const k of order.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(k)
    return next
  }
  if (!next.delete(id)) next.add(id)
  return next
}

/** The sum of the word spans of `ids`, in ms (the engine's measure of speech). */
export function speechMs(segments: readonly Segment[], ids: ReadonlySet<number> | readonly number[]): number {
  const want = ids instanceof Set ? ids : new Set(ids as readonly number[])
  let ms = 0
  for (const s of segments) {
    if (!want.has(s.id)) continue
    for (const w of s.words) ms += Number(w.t1_ms) - Number(w.t0_ms)
  }
  return ms
}

/** Why `ids` cannot be enrolled, or `null` when they can. */
export function enrollBlock(segments: readonly Segment[], ids: ReadonlySet<number> | readonly number[]): string | null {
  const want = ids instanceof Set ? ids : new Set(ids as readonly number[])
  const tracks = new Set(segments.filter((s) => want.has(s.id)).map((s) => s.track))
  if (tracks.size > 1) return 'pick segments from one track'
  if (speechMs(segments, want) < MIN_SPEECH_MS) return 'pick at least 1 s of speech'
  return null
}

/** `enrollBlock` for one segment alone (one track is given): under 1 s of its own word spans, or `null`. O(words). */
export function rowBlock(seg: Segment): string | null {
  let ms = 0
  for (const w of seg.words) ms += Number(w.t1_ms) - Number(w.t0_ms)
  return ms < MIN_SPEECH_MS ? 'pick at least 1 s of speech' : null
}

/** `23 s`, or `0.8 s` under ten seconds. */
export const secondsText = (ms: number): string => `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`

/** The success toast for `meeting_speaker_enroll`, with the cap noted when the engine used less than it was given. */
export function enrolledToast(r: Enrolled, target: EnrollTarget): string {
  const used = secondsText(r.speech_ms)
  const head = target.kind === 'New' ? `Enrolled ${r.profile.name} from ${used} of speech` : `Added ${used} to ${r.profile.name} (${r.profile.samples} samples)`
  return r.available_ms > r.speech_ms ? `${head} (using ${used} of ${secondsText(r.available_ms)})` : head
}

/** The profile named in the engine's "a speaker named X is already enrolled" refusal, or `null`. */
export const alreadyEnrolled = (message: string): string | null => /a speaker named (.+) is already enrolled/.exec(message)?.[1] ?? null
