import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { LiveBadge, liveBadge } from './LiveBadge'

test('§7.4 item 6: the island badge shows only with something active, the main window hidden and no meeting on the island', () => {
  expect(liveBadge(2, false, false)).toBe(2)
  // Nothing active: no badge, whatever the windows.
  expect(liveBadge(0, false, false)).toBeNull()
  // The main window is shown: its strip says it already.
  expect(liveBadge(1, true, false)).toBeNull()
  // A meeting owns the island.
  expect(liveBadge(1, false, true)).toBeNull()
})

test('§7.4 item 6: the badge reads LIVE N with a Stop button', () => {
  const html = renderToStaticMarkup(h(LiveBadge, { count: 3 }))
  expect(html).toContain('LIVE 3')
  expect(html).toContain('>Stop</button>')
})
