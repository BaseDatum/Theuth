import { describe, expect, test } from 'vitest'
import fixtures from '../../../../agent/tests/fixtures/attach_markers.json'
import { type AttachRef, KIND_LABEL, KINDS, compose, isKind, marker, parse, subtitle, terminalRef, title, toRef } from './attach'
import { attachRef, expand } from './pastes'

type FixtureCase = { name: string; input: string; refs: AttachRef[]; body: string; canonical?: boolean }
const cases = (fixtures as { cases: FixtureCase[] }).cases

describe('shared attach_markers.json fixtures (with terminal)', () => {
  test.each(cases.map((c) => [c.name, c] as const))('%s', (_, c) => {
    expect(parse(c.input)).toEqual({ refs: c.refs, body: c.body })
    if (c.refs.length > 0 && !c.input.startsWith('[doc ') && c.canonical !== false) {
      const lines = c.input.split('\n').slice(0, c.refs.length)
      c.refs.forEach((r, i) => {
        if (lines[i]?.startsWith('[attached ')) expect(marker(r)).toBe(lines[i])
      })
    }
  })
})

describe('terminal attach kind and marker', () => {
  test('registers kind "terminal" with label "Terminal" while preserving every existing kind', () => {
    expect(isKind('terminal')).toBe(true)
    expect(KIND_LABEL.terminal).toBe('Terminal')
    for (const existing of ['note', 'knowledge', 'artifact', 'session', 'meeting', 'notebook', 'source', 'whiteboard'] as const) {
      expect(KINDS).toContain(existing)
      expect(isKind(existing)).toBe(true)
      expect(KIND_LABEL[existing].length).toBeGreaterThan(0)
    }
  })

  test('serialises and parses the exact terminal marker line', () => {
    const terminal = {
      engine: 'mesh-node-1',
      shell: 'term-a1b2c3d4',
      name: 'mac-studio terminal',
      node: 'mac-studio',
    }
    const ref = terminalRef(terminal)
    expect(ref).toEqual({
      kind: 'terminal',
      id: 'mesh-node-1:term-a1b2c3d4',
      title: 'mac-studio terminal',
      meta: 'mac-studio',
    })
    const line = marker(ref)
    expect(line).toBe('[attached terminal mesh-node-1:term-a1b2c3d4 "mac-studio terminal" · mac-studio]')
    expect(parse(`${line}\ncheck the logs`)).toEqual({
      refs: [ref],
      body: 'check the logs',
    })
    expect(toRef({ kind: 'terminal', terminal })).toEqual(ref)
    expect(title({ kind: 'terminal', terminal })).toBe('mac-studio terminal')
    expect(subtitle({ kind: 'terminal', terminal })).toBe('mac-studio')
  })

  test('compose and expand emit one exact marker line per terminal and dedupe identical terminals', () => {
    const t1 = { engine: 'eng-1', shell: 'term-1111', name: 'laptop terminal', node: 'laptop' }
    const t2 = { engine: 'eng-2', shell: 'term-2222', name: 'build-box terminal', node: 'build-box' }
    const r1 = terminalRef(t1)
    const r2 = terminalRef(t2)

    let held = attachRef(r1, new Map())
    // Attaching the same terminal twice is a no-op (one marker per terminal).
    held = attachRef(r1, held)
    held = attachRef(r2, held)

    expect(held.size).toBe(2)
    expect(expand('run tests', held)).toEqual({
      text: [
        '[attached terminal eng-1:term-1111 "laptop terminal" · laptop]',
        '[attached terminal eng-2:term-2222 "build-box terminal" · build-box]',
        'run tests',
      ].join('\n'),
      files: [],
    })
  })

  test('existing kinds still round-trip alongside terminal', () => {
    const refs: AttachRef[] = [
      { kind: 'note', id: 'doc-1', title: 'Notes', meta: 'note' },
      { kind: 'knowledge', id: 'facts/x@abc', title: 'Facts', meta: 'revision abc' },
      { kind: 'artifact', id: 'toolu_1', title: 'Report', meta: '' },
      { kind: 'session', id: 'sess-1', title: 'Debug', meta: 'chat' },
      { kind: 'meeting', id: 'meet-1', title: 'Sync', meta: '10 min' },
      { kind: 'whiteboard', id: 'wb-1', title: 'Board', meta: '2 cards' },
      { kind: 'terminal', id: 'node-1:term-9', title: 'mac terminal', meta: 'mac' },
    ]
    const body = 'look at all of these'
    expect(parse(compose(refs, body))).toEqual({ refs, body })
  })
})
