import { expect, test } from 'vitest'
import { LENGTH, PEAK, chime } from './cue'

// A fake `BaseAudioContext` recording every oscillator's window and every gain target.
function fake() {
  const stops: number[] = []
  const starts: number[] = []
  const gains: number[] = []
  const destination = {}
  const connect = (n: unknown) => n
  const ctx = {
    currentTime: 10,
    destination,
    createOscillator: () => ({
      connect,
      type: '',
      frequency: { value: 0 },
      start: (t: number) => starts.push(t),
      stop: (t: number) => stops.push(t),
    }),
    createGain: () => ({
      connect,
      gain: {
        setValueAtTime: (v: number) => gains.push(v),
        linearRampToValueAtTime: (v: number) => gains.push(v),
        exponentialRampToValueAtTime: (v: number) => gains.push(v),
      },
    }),
  }
  return { ctx: ctx as unknown as BaseAudioContext, starts, stops, gains }
}

test('the chime is two notes, all within 250 ms of `when`, never louder than PEAK', () => {
  const { ctx, starts, stops, gains } = fake()
  chime(ctx)
  expect(starts).toHaveLength(2)
  expect(Math.min(...starts)).toBe(10)
  expect(Math.max(...stops) - 10).toBeLessThanOrEqual(LENGTH)
  expect(Math.max(...gains)).toBeLessThanOrEqual(PEAK)
  expect(PEAK).toBeLessThan(0.1)
})
