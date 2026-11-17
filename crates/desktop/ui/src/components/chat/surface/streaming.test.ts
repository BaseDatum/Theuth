import { readFileSync } from 'node:fs'
import type { Spec } from '@json-render/core'
import { expect, test } from 'vitest'
import { parseSpec } from './parse'
import { holdFenceOpener, openSpec } from './streaming'

const RUST = new URL('../../../../../../surface/fixtures/json-render/', import.meta.url)
const fixture = (name: string) => readFileSync(new URL(`${name}.json`, RUST), 'utf8').trimEnd()
const prefixes = (text: string) => Array.from({ length: text.length + 1 }, (_, i) => text.slice(0, i))

const BODIES = ['valid-metrics', 'valid-key-value', 'valid-kanban', 'valid-graph-extended'].map(fixture)
const REPLY = [
  'Here is where the cluster stands after the rollout: `kubectl rollout status` is green.',
  '',
  '```json-render',
  BODIES[0],
  '```',
  '',
  'p99 held at 180 ms; the old graph:',
  '',
  '```mermaid',
  'graph TD',
  '  A --> B',
  '```',
  '',
  'Nothing needs rolling back.',
].join('\n')

test('no prefix of a streaming reply hands streamdown a partial fence opener; the line shows with its newline', () => {
  for (const p of prefixes(REPLY)) {
    const held = holdFenceOpener(p)
    expect(p.startsWith(held), JSON.stringify(p.slice(-20))).toBe(true)
    // Only the unfinished last line is ever held back.
    expect(p.slice(held.length)).not.toContain('\n')
    expect(held, JSON.stringify(p.slice(-20))).not.toMatch(/(^|\n) {0,3}[`~][^\n]*$/)
    if (p.endsWith('\n')) expect(held).toBe(p)
  }
})

test('the S4 split: "```js" | "on-ren" | "der\\n{…" shows no fence until the opener line is complete', () => {
  const at = REPLY.indexOf('```json-render')
  for (const cut of [at + 1, at + 2, at + 3, at + 5, at + 11, at + 14]) expect(holdFenceOpener(REPLY.slice(0, cut))).toBe(REPLY.slice(0, at))
  expect(holdFenceOpener(REPLY.slice(0, at + 15))).toBe(REPLY.slice(0, at + 15))
  expect(holdFenceOpener(`${REPLY.slice(0, at + 15)}{"ro`)).toBe(`${REPLY.slice(0, at + 15)}{"ro`)
})

test('prose that starts a line with inline code or tildes is never held', () => {
  expect(holdFenceOpener('ok\n`cargo te')).toBe('ok\n`cargo te')
  expect(holdFenceOpener('ok\n~~strike')).toBe('ok\n~~strike')
  expect(holdFenceOpener('ok\n``')).toBe('ok\n')
  expect(holdFenceOpener('~~~py')).toBe('')
})

test('an open fence never falls back to nothing once a spec has parsed, over every prefix of real bodies', () => {
  for (const body of BODIES) {
    let kept: Spec | null = null
    let seen = false
    for (const p of prefixes(body)) {
      kept = openSpec(p, kept)
      if (kept) seen = true
      else expect(seen, `${body.slice(0, 30)}… at ${p.length}`).toBe(false)
    }
    expect(seen).toBe(true)
    // The whole body's spec is the closed fence's.
    const closed = parseSpec(body)
    expect('spec' in closed && closed.spec.root).toBe(kept?.root)
  }
})
