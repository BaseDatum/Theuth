import { expect, test } from 'vitest'
import type { PathCandidate } from "@/rpc/bindings/method.agent.output"
import { type PathPopup, accept, pathKey, pathTrigger, step } from './path-complete'

/** `|` marks the caret. */
const at = (s: string): [string, number] => [s.replace('|', ''), s.indexOf('|')]

// The same vectors as `agent::pathac::tests::trigger_vectors`.
test('path trigger vectors', () => {
  const yes: [string, [number, number]][] = [
    ['~/De|', [0, 4]],
    ['see ~/Develop/th|', [4, 16]],
    ['./src/|', [0, 6]],
    ['$HOME/.th|', [0, 9]],
    ['../x|', [0, 4]],
    ['/|', [0, 1]],
    ['~|', [0, 1]],
    ['~bob/x|', [0, 6]],
    ['open "~/My Docs/f|', [6, 17]],
    ['a\nb /etc/ho|', [4, 11]],
    ['~/My\\ Docs/|', [0, 11]],
    ['```\n~/x\n```\nsee ~/De|', [16, 20]],
  ]
  for (const [s, [start, end]] of yes) {
    const [text, caret] = at(s)
    expect(pathTrigger(text, caret), s).toEqual({ start, end })
  }
  const no = [
    'no/trigger here|',
    'no/trigger| here',
    '```\n~/De|',
    '```sh\nls ~/x\n```\n```\n/etc|',
    '@note|',
    '@~/x|',
    'hello|',
    '$HOME|',
    '~bob|',
    'C:\\Users\\|',
    '|',
  ]
  for (const s of no) {
    const [text, caret] = at(s)
    expect(pathTrigger(text, caret), s).toBeNull()
  }
  expect(pathTrigger('C:\\Users\\', 9, true)).toEqual({ start: 0, end: 9 })
  expect(pathTrigger('D:/x', 4, true)).toEqual({ start: 0, end: 4 })
})

const cand = (insert: string, is_dir: boolean): PathCandidate => ({
  insert,
  display: insert.slice(insert.lastIndexOf('/', insert.length - 2) + 1),
  is_dir,
})

test('popup keys, cycling and accept insert only the remainder', () => {
  const popup: PathPopup = {
    start: 4,
    end: 8,
    items: [cand('~/Develop/', true), cand('~/Desktop/', true), cand('~/Dfile', false)],
    truncated: false,
    selected: 0,
  }
  const key = (k: string, shift = false, atEnd = true) => pathKey({ key: k, shiftKey: shift }, atEnd)
  expect(key('ArrowDown')).toBe('down')
  expect(key('ArrowUp')).toBe('up')
  expect(key('Tab')).toBe('accept')
  expect(key('Tab', true)).toBe('up')
  expect(key('ArrowRight')).toBe('accept')
  expect(key('ArrowRight', false, false)).toBeNull()
  expect(key('Escape')).toBe('close')
  expect(key('Enter')).toBeNull()
  expect(key('a')).toBeNull()

  expect(step(popup, -1).selected).toBe(2)
  expect(step(step(popup, 1), 1).selected).toBe(2)
  expect(step(step(popup, 1), -1).selected).toBe(0)

  // `see ~/De` → the typed `~/` stays; the text is never the expanded home.
  const dir = accept('see ~/De', popup, popup.items[1]!)
  expect(dir).toEqual({ text: 'see ~/Desktop/', caret: 14 })
  // A directory keeps completing: the new token is the whole accepted path.
  expect(pathTrigger(dir.text, dir.caret)).toEqual({ start: 4, end: 14 })
  // Text after the caret survives.
  expect(accept('see ~/De later', popup, popup.items[2]!)).toEqual({ text: 'see ~/Dfile later', caret: 11 })
})
