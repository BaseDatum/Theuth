import { expect, test } from 'vitest'
import { CONSOLE, SIZE_CHANGED } from '@/lib/page-bootstrap'
import { LOAD_MS, MEASURE_MS, SETTLE_MS, answerOf, begin, dueAt, onLoad, onPost } from './page-preview'
import type { PagePreviewAnswer } from '@/rpc/bindings/method.artifact.input'

const size = (height: unknown) => ({ jsonrpc: '2.0', method: SIZE_CHANGED, params: { height } })
const line = (level: string, text: string) => ({ jsonrpc: '2.0', method: CONSOLE, params: { level, text } })

test('settles 300 ms after the last post once loaded, never past 6 s after load', () => {
  let r = begin(1000)
  expect(dueAt(r)).toBe(1000 + LOAD_MS)
  r = onLoad(r, 1200)
  expect(dueAt(r)).toBe(1200 + SETTLE_MS)
  r = onPost(r, size(400), 1500)
  expect(dueAt(r)).toBe(1500 + SETTLE_MS)
  // A post before `load` (DOMContentLoaded) counts from `load`, not from itself.
  const early = onLoad(onPost(begin(0), size(100), 50), 500)
  expect(dueAt(early)).toBe(500 + SETTLE_MS)
  // A page that keeps resizing runs into the measure budget.
  r = onPost(r, size(900), 1200 + MEASURE_MS + 1000)
  expect(dueAt(r)).toBe(1200 + MEASURE_MS)
  // A second `load` (the page navigated itself) does not move the clock.
  expect(onLoad(r, 9000).loadedAt).toBe(1200)
})

test('collects sizes and console lines with the shared parsers; other posts change nothing', () => {
  let r = begin(0)
  const same = onPost(r, { jsonrpc: '2.0', method: 'ui/open-link', params: { url: 'https://x' } }, 10)
  expect(same).toBe(r)
  expect(onPost(r, size(-1), 10)).toBe(r)
  expect(onPost(r, size('400'), 10)).toBe(r)
  r = onPost(r, size(412.4), 10)
  r = onPost(r, line('error', 'boom'), 20)
  r = onPost(r, line('shout', 'nope'), 30)
  expect(r.height).toBe(412.4)
  expect(r.lastPost).toBe(20)
  expect(r.console).toEqual([{ level: 'error', text: 'boom' }])
})

test('answers the last height clamped 80–2000, the floor when nothing was posted, loaded only after load', () => {
  const r = onPost(onLoad(begin(0), 1), size(412.4), 2)
  const height = (a: PagePreviewAnswer) => (a.kind === 'done' ? a.content_height : null)
  expect(answerOf(r, 'desktop')).toEqual({ kind: 'done', content_height: 412, console: [], loaded: true, client: 'desktop' })
  expect(height(answerOf(onPost(r, size(5000), 3), 'web'))).toBe(2000)
  expect(height(answerOf(onPost(r, size(3), 3), 'web'))).toBe(80)
  const never = onPost(begin(0), line('log', 'hi'), 5)
  expect(answerOf(never, 'phone')).toEqual({ kind: 'done', content_height: 80, console: [{ level: 'log', text: 'hi' }], loaded: false, client: 'phone' })
})
