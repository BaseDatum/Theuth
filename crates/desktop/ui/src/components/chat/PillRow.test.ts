import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test } from 'vitest'
import { PillRow } from './SkillPicker'

let host: HTMLDivElement | null = null
afterEach(() => host?.remove())

function render(props: Parameters<typeof PillRow>[0]): HTMLElement {
  host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  act(() => root.render(h(PillRow, props)))
  return host
}

const noop = () => {}

test('215 G5: skill chips and the suffix pill share one left-aligned wrapping row', () => {
  const el = render({
    skills: ['ponytail', 'caveman'],
    onRemoveSkill: noop,
    suffix: { id: 'sx', name: 'Be brief', text: 'brief', scope: 'once' },
    onClearSuffix: noop,
  })
  const pills = [...el.querySelectorAll('[data-skill-chip], [data-suffix]')]
  expect(pills.map((p) => p.getAttribute('data-skill-chip') ?? p.getAttribute('data-suffix'))).toEqual(['ponytail', 'caveman', 'sx'])
  const row = pills[0]?.parentElement
  for (const p of pills) expect(p.parentElement).toBe(row)
  expect(row?.classList.contains('flex-wrap')).toBe(true)
  expect(row?.className).not.toMatch(/justify-end/)
  for (const p of pills) expect(p.className).toMatch(/max-w-\[240px\]/)
})

test('no pill, no row', () => {
  expect(render({ skills: [], onRemoveSkill: noop, suffix: null, onClearSuffix: noop }).innerHTML).toBe('')
})
