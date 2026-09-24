import { expect, test } from 'vitest'
import { STEPS, lifecycle } from './ControlCenter'

test('no strip while live, before the recording ended, or for an unfinished start', () => {
  expect(lifecycle(undefined, null, null, true)).toBeNull()
  expect(lifecycle('Recording', null, null, false)).toBeNull()
  expect(lifecycle('Failed', null, 'no mic', false)).toBeNull()
  expect(lifecycle('Stopping', null, null, true)).toBeNull()
})

test('pipeline stages are busy, the store stage wins over the manifest', () => {
  expect(lifecycle({ Post: 'Diarizing' }, null, null, true)).toEqual({ current: 1, busy: true, failure: null, action: null })
  expect(lifecycle('Review', 'Aligning', null, true)).toEqual({ current: 2, busy: true, failure: null, action: null })
})

test('review waits on the user; a summary failure offers a retry', () => {
  expect(lifecycle('Review', null, null, true)).toEqual({ current: STEPS.indexOf('Review'), busy: false, failure: null, action: null })
  expect(lifecycle('Review', null, 'summary did not complete: its turn did not finish within 10 minutes', true)).toEqual({
    current: STEPS.indexOf('Review'),
    busy: false,
    failure: 'summary did not complete: its turn did not finish within 10 minutes',
    action: 'summarize',
  })
})

test('summarizing spins, done is the last step, failed offers the pipeline', () => {
  expect(lifecycle('Summarizing', null, null, true)).toEqual({ current: STEPS.indexOf('Summarizing'), busy: true, failure: null, action: null })
  expect(lifecycle('Done', null, null, true)).toEqual({ current: STEPS.length - 1, busy: false, failure: null, action: null })
  expect(lifecycle('Failed', null, 'align: no words', true)).toEqual({ current: null, busy: false, failure: 'align: no words', action: 'post_retry' })
})
