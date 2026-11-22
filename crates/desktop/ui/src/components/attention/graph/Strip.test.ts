// 208 §10.2: the collapsed strip's text at the fixture instant.
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { GRAPH_FIXTURE, GRAPH_FIXTURE_ATTENTION as ATT, GRAPH_FIXTURE_NOW as T0 } from './fixture'
import { fromSnapshot } from './reducer'
import { stripCounts } from './selectors'
import { Strip } from './Strip'
import { TTL_DEFAULTS, visibleGraph } from './ttl'

const counts = stripCounts(visibleGraph(fromSnapshot(GRAPH_FIXTURE), ATT, TTL_DEFAULTS, T0))
const render = (over: Partial<Parameters<typeof Strip>[0]> = {}) =>
  renderToStaticMarkup(createElement(Strip, { counts, older: GRAPH_FIXTURE.older, subtree: null, expanded: false, onToggle: () => undefined, ...over })).replace(/<[^>]+>/g, '')

test('T0 text and "+3 older trees"', () => {
  const t = render()
  expect(t).toContain('4 need you · 11 working · 14 finished')
  expect(t).toContain('+3 older trees')
})

test('no older stub, no "older"; the subtree chip mirrors the filter', () => {
  expect(render({ older: null })).not.toContain('older')
  expect(render({ subtree: 'ops desk' })).toContain('In ops desk')
})
