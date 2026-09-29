// 76 TALK-UI-A A2: the turn cue. The user stopped talking and the model took the turn — one soft two-tone chime,
// generated on the page's `AudioContext` (no asset), quiet, ≤ 250 ms. Never on a barge-in resume, never while
// the user speaks: the store plays it when `TalkState` leaves `listening` for `thinking` after `Submitted` (`lib/talk.ts`).

/** Peak gain: well under speech level. */
export const PEAK = 0.06
/** The two notes: `[hz, start, duration]` seconds from `when`; E5 then A5, overlapping 20 ms. */
export const TONES: readonly (readonly [number, number, number])[] = [
  [659.25, 0, 0.12],
  [880, 0.1, 0.13],
]
/** The whole cue fits in this window. */
export const LENGTH = 0.25

export function chime(ctx: BaseAudioContext, when = ctx.currentTime): void {
  for (const [hz, at, dur] of TONES) {
    const t = when + at
    const osc = ctx.createOscillator()
    const gain = ctx.createGain()
    osc.type = 'sine'
    osc.frequency.value = hz
    gain.gain.setValueAtTime(0, t)
    gain.gain.linearRampToValueAtTime(PEAK, t + 0.01)
    gain.gain.exponentialRampToValueAtTime(0.001, t + dur)
    osc.connect(gain).connect(ctx.destination)
    osc.start(t)
    osc.stop(t + dur)
  }
}
