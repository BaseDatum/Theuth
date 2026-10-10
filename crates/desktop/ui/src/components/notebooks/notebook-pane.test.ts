import { expect, test } from 'vitest'
import { paneMode } from './notebook-pane'

test('157 D4 paneMode: no ?note is the list; a member note or the just-created one opens; a foreign id falls back to the list', () => {
  expect(paneMode(undefined, ['n1', 'n2'], null)).toEqual({ kind: 'list' })
  expect(paneMode('', ['n1'], null)).toEqual({ kind: 'list' })
  expect(paneMode('n1', ['n1', 'n2'], null)).toEqual({ kind: 'note', id: 'n1' })
  expect(paneMode('n9', ['n1', 'n2'], null)).toEqual({ kind: 'list' })
  // Just created here: the items list has not refetched yet.
  expect(paneMode('n9', ['n1', 'n2'], 'n9')).toEqual({ kind: 'note', id: 'n9' })
  // Items still loading: trust the URL rather than flash the list.
  expect(paneMode('n1', undefined, null)).toEqual({ kind: 'note', id: 'n1' })
})
