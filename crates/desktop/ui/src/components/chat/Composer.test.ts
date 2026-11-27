import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { type Active, finish } from '@/lib/dictation'
import { dictationCommands, handsOff, pastedFiles, placeholderFor, SendButton, spokenSend, type Tail } from './Composer'
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

test('171F B7: queued chips, then the strip, then the input box (the strip sits on the box, the chips above it)', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const at = (s: string) => src.indexOf(s)
  expect(at('<QueuedChips')).toBeGreaterThan(0)
  expect(at('<QueuedChips')).toBeLessThan(at('{strip}'))
  expect(at('{strip}')).toBeLessThan(at('ref={box}'))
})

test('183 §12.1 / mock panel 3: a chat owned elsewhere says where the message runs, a steer too; talk and private keep theirs', () => {
  const idle = { running: false, exports: false }
  // The name never breaks: a word joiner after its hyphen, no-break spaces (a 430 px box wrapped `cloud-` / `1`).
  expect(placeholderFor({ ...idle, runsOn: 'cloud-1' })).toBe('Message — runs on cloud-\u20601')
  expect(placeholderFor({ ...idle, running: true, runsOn: 'cloud-1' })).toBe('Message — runs on cloud-\u20601')
  expect(placeholderFor({ ...idle, runsOn: "Sky's MacBook", short: true })).toBe("Runs on Sky's\u00a0MacBook")
  expect(placeholderFor(idle)).toBe('…')
  expect(placeholderFor({ ...idle, running: true })).toBe('Send to steer…')
  expect(placeholderFor({ running: true, exports: true, runsOn: 'cloud-1' })).toBe('Send privately…')
  expect(placeholderFor({ ...idle, skill: 'pdf', runsOn: 'cloud-1' })).toBe('Words for pdf (optional)')
  expect(placeholderFor({ ...idle, readOnly: true, runsOn: 'cloud-1' })).toBe('Talking — speak, or stop the talk to type')
})

test('215 C2: inside the data-composer box the text comes first, then the toolbar row (Add … MCP · context · shield … model … Send)', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const at = (s: string) => src.indexOf(s)
  const order = ['data-composer=""', '<Textarea', 'aria-label="Add"', '<McpMenu', '<ContextButton', '<ApprovalShield', '<ModelSlot', '<MicButton', '<SendButton ', '{suggestion &&']
  for (const s of order) expect(at(s), s).toBeGreaterThan(0)
  for (let i = 1; i < order.length; i++) expect(at(order[i - 1]!), `${order[i - 1]} before ${order[i]}`).toBeLessThan(at(order[i]!))
  expect(src).not.toMatch(/rounded-\[28px\]/)
})

test('Stop→Send draws in only when this session\'s running turn ends, never on mount, a switch, or reduced motion', () => {
  // The Composer's render loop: each change of tail asks `handsOff`; the first tail is the mount and asks nothing.
  const run = (tails: Tail[], reduce = false) => tails.slice(1).map((t, i) => handsOff(tails[i]!, t, reduce))
  const stop = (session: string | null = 'a'): Tail => ({ session, button: 'stop' })
  const send = (session: string | null = 'a'): Tail => ({ session, button: 'send' })
  expect(run([send(), stop(), send()])).toEqual([false, true])
  expect(run([stop(), send()], true)).toEqual([false])
  expect(run([stop('a'), send('b')])).toEqual([false])
  // A voice note holding the row hands it back as Send: not a turn ending.
  expect(run([stop(), { session: 'a', button: null }, send()])).toEqual([false, false])
  expect(run([send(), send()])).toEqual([false])
})

test('SendButton carries the draw-in classes only while `arrive`, at rest otherwise', () => {
  const paint = (arrive?: boolean) =>
    renderToStaticMarkup(createElement(SendButton, { disabled: true, waiting: null, onSend: () => {}, arrive }))
  const drawn = paint(true)
  for (const c of ['send-arrive-fill', 'send-arrive-square', 'send-arrive-ring', 'send-arrive-bar']) expect(drawn).toContain(c)
  expect(drawn).toContain('bg-danger')
  expect(paint()).not.toContain('send-arrive')
  expect(paint(false)).not.toContain('send-arrive')
})

test('the draw-in is CSS-gated: with reduced motion the overlay is hidden and every arrive default is the resting mark', () => {
  const css = readFileSync(new URL('../../styles.css', import.meta.url), 'utf8')
  const gated = css.slice(css.indexOf('.send-arrive-fill {'), css.indexOf('@keyframes send-arrive-square'))
  const [rest, motion] = gated.split('@media (prefers-reduced-motion: no-preference)')
  expect(rest).toMatch(/\.send-arrive-fill \{\s+display: none;/)
  expect(rest).not.toContain('animation')
  for (const c of ['send-arrive-square', 'send-arrive-ring', 'send-arrive-bar']) expect(motion).toMatch(new RegExp(`\\.${c} \\{\\s+animation: ${c} `))
})

test('attach file allows any file type in both the hidden input and native dialog', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  expect(src).not.toContain('accept="image/*,application/pdf"')
  expect(src).not.toContain('Images and PDF')
  expect(src).not.toMatch(/extensions:\s*\[[^\]]*'pdf'/)
  expect(src).toMatch(/<input[\s\S]*?ref=\{fileInput\}[\s\S]*?type="file"[\s\S]*?multiple/)
  expect(src).toContain('pickFiles({ multiple: true })')
})

test('pastedFiles attaches files of any type and leaves normal text pastes untouched', () => {
  const mk = (name: string, type: string) => new File(['x'], name, { type })
  const item = (file: File) => ({ kind: 'file' as const, getAsFile: () => file })
  const str = () => ({ kind: 'string' as const, getAsFile: () => null })

  const png = mk('shot.png', 'image/png')
  const zip = mk('bundle.zip', 'application/zip')
  const md = mk('notes.md', 'text/markdown')
  const html = mk('page.html', 'text/html')
  const svg = mk('icon.svg', 'image/svg+xml')
  const makefile = mk('Makefile', '')

  expect(pastedFiles({ items: [item(png)] })).toEqual([png])
  expect(pastedFiles({ items: [str(), item(zip), item(md), item(html), item(svg), item(makefile)] })).toEqual([zip, md, html, svg, makefile])
  expect(pastedFiles({ items: [], files: [zip] })).toEqual([zip])
  expect(pastedFiles({ items: [str(), str()], files: [] })).toEqual([])
})
