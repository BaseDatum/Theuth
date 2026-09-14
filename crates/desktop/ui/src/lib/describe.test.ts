import { expect, test } from 'vitest'
import { describe, resultLine } from './describe'

test('shell shows the command, continuation lines indented', () => {
  expect(describe('shell', { command: ' ls -la\nwc -l \n', timeout: 5 })).toEqual(['$ ls -la', '  wc -l'])
})

test('key: value per argument, object order kept', () => {
  expect(describe('read_file', { path: 'src/a.rs', lines: [1, 20], all: false })).toEqual([
    'path: src/a.rs',
    'lines: [1,20]',
    'all: false',
  ])
})

test('long or multi-line strings collapse to their size; long json is cut', () => {
  expect(describe('write_file', { path: 'x', content: 'héllo\nworld\n' })).toEqual([
    'path: x',
    'content: (2 lines, 13 bytes)',
  ])
  expect(describe('t', { s: 'a'.repeat(121) })).toEqual(['s: (1 lines, 121 bytes)'])
  const [line] = describe('t', { v: Array.from({ length: 60 }, (_, i) => i) })
  expect(line).toHaveLength(`v: `.length + 120 + 1)
  expect(line?.endsWith('…')).toBe(true)
})

test('null is empty, a non-object is its json', () => {
  expect(describe('t', null)).toEqual([])
  expect(describe('t', [1, 2])).toEqual(['[1,2]'])
})

test('resultLine', () => {
  expect(resultLine('ok')).toBe('ok')
  expect(resultLine('a\nb\nc\n')).toBe('a  (+2 lines)')
})
