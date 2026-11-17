import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { ContextItem, ContextKind, ContextTarget } from '@/rpc/bindings/method.agent.output'
import type * as Rpc from '@/lib/rpc'
import type * as Conn from '@/store/connection'
import { empty } from '@/store/reduce'
import { active } from '@/store/engine-scope'
import { useSessions } from '@/store/sessions'
import { Pick, ReviewBody } from './ContextPicker'
import { catalog, key } from './shake'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ get_attachment: () => Promise.withResolvers<string>().promise }),
}))
vi.mock('@/store/connection', async (original) => ({
  ...(await original<typeof Conn>()),
  useConnection: (pick: (s: unknown) => unknown) => pick({ info: { data_dir: '/dd' }, api: null, discovery: { phase: 'legacy' } }),
}))

const S = 's1'
const m = (seq: number): ContextTarget => ({ kind: 'message', seq })
const b = (seq: number, part: number): ContextTarget => ({ kind: 'binary', owner: { kind: 'message', seq }, part, image: null })
const item = (target: ContextTarget, kind: ContextKind, group: string, extra: Partial<ContextItem> = {}): ContextItem => ({
  target,
  kind,
  group,
  label: `label ${key(target)}`,
  tokens: 100,
  remove_with: [],
  ...extra,
})
const items = [
  ...Array.from({ length: 6 }, (_, i) => item(b(10 + i, 0), 'image', 'screenshot', { detail: 'PNG · 1568×1103' })),
  item(b(1, 0), 'image', 'attachment', { label: 'whiteboard.jpg', detail: 'JPEG' }),
  item(m(20), 'tool', 'python'),
  item(m(30), 'user', 'user', { label: '“go for it”' }),
  item(m(31), 'assistant', 'assistant'),
]
const cats = catalog(items)
// Server rendering reads zustand's initial state: the sessions store answers its live state.
Object.assign(active().sessions, { getInitialState: active().sessions.getState })
const pick = (selected: string[], open: string[]) =>
  renderToStaticMarkup(createElement(Pick, { session: S, cats, selected: new Set(selected), open: new Set(open), onOpen: () => {}, onChange: () => {} }))
const checkbox = (html: string, label: string) => html.match(new RegExp(`<input[^>]*aria-label="${label}"[^>]*>`))?.[0]

test('Attachments & output categories have a select-all; Conversation categories do not', () => {
  const html = pick([], ['user'])
  expect(checkbox(html, 'Select all Images')).toBeDefined()
  expect(checkbox(html, 'Select all Tool output')).toBeDefined()
  expect(html).not.toContain('Select all Your messages')
  expect(html).not.toContain('Select all My replies')
  expect(html).toContain('pick items · no select-all')
  // Opened, a Conversation category's rows are ticked one by one.
  expect(checkbox(html, '“go for it”')).toBeDefined()
})

test('a partly selected category and group render mixed with n/N; an open group shows four items then Show N more', () => {
  const html = pick([key(b(10, 0)), key(b(11, 0))], ['image'])
  expect(checkbox(html, 'Select all Images')).toContain('aria-checked="mixed"')
  expect(checkbox(html, 'Select all Screenshots')).toContain('aria-checked="mixed"')
  expect(checkbox(html, 'Select all Attachments')).not.toContain('checked')
  expect(html).toContain('>2/7<')
  expect(html).toContain('>2/6<')
  expect(html).toContain('6 screenshots · 1 attachments')
  expect(html.match(/aria-label="label m1[0-5]\/0"/g)).toHaveLength(4)
  expect(html).toContain('Show 2 more')
  expect(checkbox(html, 'label m10/0')).toContain('checked=""')
  // No preview path in the loaded transcript: the format badge.
  expect(html).toContain('>PNG<')
  // A closed category renders no item rows.
  expect(html).not.toContain('label m20')
})

test('an image with a loaded attachment path gets a thumbnail instead of its badge', () => {
  useSessions.setState({
    sessions: {
      [S]: {
        ...empty(S),
        entries: [
          {
            id: 'e1',
            kind: 'user',
            text: '',
            context: { target: m(1), active: true, removed: false, remove_with: [], attachments: [{ target: b(1, 0), name: 'whiteboard.jpg', mime: 'image/jpeg', path: '/dd/w.jpg', removed: false }] },
          },
        ] as never,
      },
    },
  })
  const html = pick([], ['image'])
  const row = html.slice(html.indexOf('aria-label="whiteboard.jpg"'))
  expect(row.slice(0, row.indexOf('whiteboard.jpg</span>'))).not.toContain('>JPEG<')
  expect(html).toContain('animate-pulse')
})

test('review: grouped lines with tokens, the companions, the three warnings in the contract wording, and the retention line', () => {
  const all = [
    item({ kind: 'binary', owner: { kind: 'head', epoch: 1 }, part: 0, image: null }, 'image', 'summary', { tokens: 1600 }),
    item({ kind: 'fence', owner: { kind: 'message', seq: 3 }, part: 0, index: 0 }, 'surface', 'diagram', { tokens: 350 }),
    item({ kind: 'source', owner: { kind: 'message', seq: 4 }, part: 1 }, 'surface', 'page', { tokens: 6200 }),
    item(m(5), 'tool', 'python', { tokens: 700, remove_with: [m(6)] }),
    item(m(6), 'tool', 'python', { tokens: 80, remove_with: [m(5)] }),
  ]
  const html = renderToStaticMarkup(createElement(ReviewBody, { cats: catalog(all), items: all, selected: new Set(['h1/0', 'm3#f0.0', 'm4#s1', 'm5']) }))
  expect(html).toContain('All 1 · Images')
  expect(html).toContain('~1.6k')
  expect(html).toContain('All 2 · Surfaces &amp; diagrams')
  expect(html).toContain('+ 1 protocol companion')
  expect(html).toContain('Removing it loses part of the earlier conversation.')
  expect(html).toContain('cut out of their replies; the reply text stays, with a one-line marker')
  expect(html).toContain('keep their card in your transcript; the model sees a one-line marker instead of their source')
  expect(html).toContain('There is no undo')
  // Nothing of the kind selected: no warning.
  const plain = renderToStaticMarkup(createElement(ReviewBody, { cats: catalog(all), items: all, selected: new Set(['m5']) }))
  expect(plain).not.toMatch(/Summary selected|Diagrams|Pages, apps/)
})
