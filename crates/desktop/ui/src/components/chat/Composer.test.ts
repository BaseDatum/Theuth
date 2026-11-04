import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'
import { type Active, finish } from '@/lib/dictation'
import { dictationCommands, spokenSend } from './Composer'
import { COMMANDS } from './slash'

const active = (utts: Active['utts']): Active => ({ id: 'd1', before: '', after: '', utts, levels: [], startedAt: 0, device: null })
const SEND = { command: 'send', near_miss: false } as const

/** The Composer's render loop: the landed value is the box's text from the next render on. */
function renders(auto: string | null, texts: string[]): number {
  let fired = 0
  for (const text of texts) {
    const r = spokenSend(auto, text)
    if (r.fire) fired++
    auto = r.auto
  }
  return fired
}

test('153 §7.4: "ship it send it" lands "ship it" and submits exactly once, however many renders follow', () => {
  const r = finish(active([{ text: 'ship it send it', shaped: 'ship it', final: true, command: SEND }]), false)
  expect(r).toMatchObject({ value: 'ship it', send: true })
  expect(renders(r.send ? r.value : null, ['', 'ship it', 'ship it', 'ship it'])).toBe(1)
})

test('a partial that only armed "send it", a plain stop, or Esc never submits', () => {
  const armed = finish(active([{ text: 'ship it send', shaped: 'ship it', final: false, command: SEND }]), false)
  expect(renders(armed.send ? armed.value : null, ['', 'ship it', 'ship it'])).toBe(0)
  const plain = finish(active([{ text: 'ship it', shaped: 'ship it', final: true }]), false)
  expect(renders(plain.send ? plain.value : null, ['ship it'])).toBe(0)
  const esc = finish(active([{ text: 'ship it send it', shaped: 'ship it', final: true, command: SEND }]), true)
  expect(esc).toMatchObject({ value: '', send: false, id: '' })
})

test('the user edits the landed text before it is sent: the spoken send waits for that exact value, nothing else fires', () => {
  expect(renders('ship it', ['ship it now'])).toBe(0)
  expect(spokenSend('ship it', 'ship it now')).toEqual({ fire: false, auto: 'ship it' })
})

test('dictationCommands: theuth\'s slash commands plus the harness\'s, each with its slash', () => {
  const names = dictationCommands([{ name: 'skill:ponytail' }, { name: '/ponytail-review' }])
  expect(names.slice(0, COMMANDS.length)).toEqual(COMMANDS.map((c) => c.name))
  expect(names.slice(COMMANDS.length)).toEqual(['/skill:ponytail', '/ponytail-review'])
  expect(dictationCommands(null)).toEqual(COMMANDS.map((c) => c.name))
})

test('171F B7: queued chips, then the kernel strip, then the input box (the strip sits on the box, the chips above it)', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const at = (s: string) => src.indexOf(s)
  expect(at('<QueuedChips')).toBeGreaterThan(0)
  expect(at('<QueuedChips')).toBeLessThan(at('{strip}'))
  expect(at('{strip}')).toBeLessThan(at('ref={box}'))
  const route = readFileSync(new URL('../../routes/c.$session.tsx', import.meta.url), 'utf8')
  expect(route).toContain('strip={<KernelStrip session={session} embedded />}')
})
