import { createElement as h } from 'react'
import type * as ReactDom from 'react-dom'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { QuitState } from '@/lib/quit-progress'
import { QuitOverlay, useQuit } from './QuitOverlay'

// The overlay portals into `<body>`; render it in place.
vi.mock('react-dom', async (original) => ({ ...(await original<typeof ReactDom>()), createPortal: (node: unknown) => node }))
vi.stubGlobal('document', { body: {} })

const QUIT: QuitState = {
  restart: false,
  owned: true,
  at: 1_000,
  turns: [
    { id: 'a', title: 'Refactor the parser', done: true },
    { id: 'b', title: 'Weekly report', done: false },
  ],
  companion: true,
  step: 'companion',
  forced: true,
}

test('the quit overlay markup is unchanged by the shared launch card (snapshot taken on 887e5f5f3)', () => {
  vi.spyOn(Date, 'now').mockReturnValue(4_000)
  // `renderToStaticMarkup` reads a `create` store's initial state (one object, returned by reference): preset that.
  const initial = useQuit.getInitialState()
  initial.state = QUIT
  expect(renderToStaticMarkup(h(QuitOverlay))).toMatchSnapshot()
  initial.state = { ...QUIT, step: 'closing' }
  expect(renderToStaticMarkup(h(QuitOverlay))).toMatchSnapshot()
})
