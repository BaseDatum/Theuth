import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { MeasuredContext, remember } from '@/lib/measured'
import { Reserve } from './Reserve'

const inTranscript = (id: string | null, ready: boolean, width = 736) =>
  renderToStaticMarkup(
    createElement(MeasuredContext, { value: { session: 'reserve-test', width } }, createElement(Reserve, { id, ready, className: 'box', children: 'child' })),
  )

test('reserves the remembered height while not ready; nothing once ready, unknown, outside a transcript, or id null', () => {
  remember('reserve-test', 736, 'e1/diagram-abc', 412)
  expect(inTranscript('e1/diagram-abc', false)).toBe('<div class="box grid" style="min-height:412px">child</div>')
  expect(inTranscript('e1/diagram-abc', true)).toBe('<div class="box">child</div>')
  expect(inTranscript('e1/diagram-zzz', false)).toBe('<div class="box">child</div>')
  // Another width bucket does not restore the height.
  expect(inTranscript('e1/diagram-abc', false, 400)).toBe('<div class="box">child</div>')
  expect(inTranscript(null, false)).toBe('<div class="box">child</div>')
  expect(renderToStaticMarkup(createElement(Reserve, { id: 'e1/diagram-abc', ready: false, className: 'box', children: 'child' }))).toBe('<div class="box">child</div>')
})
