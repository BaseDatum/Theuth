import { expect, test } from 'vitest'
import { band } from './ContextMeter'

test('bands at 75 / 90 / 95 % (75 §5); no limit is normal', () => {
  expect([null, 0, 74, 75, 89, 90, 95, 96, 120].map(band)).toEqual([
    'normal',
    'normal',
    'normal',
    'warning',
    'warning',
    'danger',
    'danger',
    'critical',
    'critical',
  ])
})
