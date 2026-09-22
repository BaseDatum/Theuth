import { expect, test } from 'vitest'
import type { Line } from '@/store/meeting'
import { preview } from './IslandTranscript'

const line = (track: Line['track'], text: string): Line => ({ track, t0_ms: 0, text, request: false, open: false })

test('collapsed preview: speaker label, trimmed, cut at the limit with an ellipsis', () => {
  expect(preview(line('Mic', '  right  '))).toBe('me: right')
  expect(preview(line('System', 'so the DR sync'))).toBe('them: so the DR sync')
  expect(preview(line('System', 'abcdefghij'), 5)).toBe('them: abcde…')
  expect(preview(line('System', 'abcd efghij'), 5)).toBe('them: abcd…')
  expect(preview(line('System', 'abcde'), 5)).toBe('them: abcde')
})
