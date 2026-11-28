import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'
import { excerptLines, parsePartial, parseSpec } from './parse'

const RUST = new URL('../../../../../../surface/fixtures/json-render/', import.meta.url)
const fixture = (name: string) => readFileSync(new URL(`${name}.json`, RUST), 'utf8')

const ok = (body: string) => {
  const r = parseSpec(body)
  if ('error' in r) throw new Error(r.error)
  return r
}
const err = (body: string) => {
  const r = parseSpec(body)
  if (!('error' in r)) throw new Error('expected an error')
  return r.error
}

const CARD = `{"root":"r","elements":{
  "r":{"type":"Card","props":{"title":"Release"},"children":["m","t"]},
  "m":{"type":"MetricGrid","props":{"items":[{"label":"Tests","value":"412 / 412"},{"label":"Coverage","value":"81%","trend":"up"}]}},
  "t":{"type":"Table","props":{"columns":["Check","Status"],"rows":[["clippy","pass"],["signing","pending"]]}}
}}`

test('strict body parses without repair; props/children default', () => {
  const { spec, repaired } = ok(CARD)
  expect(repaired).toBe(false)
  expect(spec.root).toBe('r')
  expect(spec.elements.r?.children).toEqual(['m', 't'])
  expect(spec.elements.t?.children).toEqual([])
  expect(ok('{"root":"s","elements":{"s":{"type":"Separator"}}}').spec.elements.s?.props).toEqual({})
})

test('trailing commas and single quotes are repaired and flagged', () => {
  expect(ok(`{"root":"r","elements":{"r":{"type":"Card","props":{"title":"A",},"children":[],},},}`).repaired).toBe(true)
  const single = ok(`{'root':'r','elements':{'r':{'type':'Heading','props':{'text':'Hi','level':1}}}}`)
  expect(single.repaired).toBe(true)
  expect(single.spec.elements.r?.props).toEqual({ text: 'Hi', level: 1 })
})

test('located errors: type, child id, prop shape, root, parents, cycle, leaf children, junk', () => {
  expect(err(`{"root":"t","elements":{"t":{"type":"DataTable"}}}`)).toMatch(/^\/elements\/t\/type: unknown component "DataTable"; catalog: .*Card.*Table/)
  expect(err(`{"root":"r","elements":{"r":{"type":"Card","children":["m","m2"]},"m":{"type":"Separator"}}}`)).toMatch(/^\/elements\/r\/children\/1: "m2"/)
  expect(err(`{"root":"m","elements":{"m":{"type":"KeyValueList","props":{"items":[{"key":"a","value":1}]}}}}`)).toMatch(/^\/elements\/m\/props\/items\/0\/value: /)
  expect(err(`{"root":"m","elements":{"m":{"type":"Text","props":{"text":"x","colour":"red"}}}}`)).toMatch(/^\/elements\/m\/props: .*colour/)
  expect(err(`{"root":"main","elements":{"r":{"type":"Separator"}}}`)).toMatch(/^\/root: "main"/)
  expect(
    err(`{"root":"r","elements":{"r":{"type":"Card","children":["c","x"]},"c":{"type":"Stack","children":["x"]},"x":{"type":"Separator"}}}`),
  ).toMatch(/^\/: element "x" has two parents \(r, c\)/)
  expect(err(`{"root":"a","elements":{"a":{"type":"Card","children":["b"]},"b":{"type":"Card","children":["a"]}}}`)).toMatch(/^\/: cycle through "a"/)
  expect(err(`{"root":"h","elements":{"h":{"type":"Heading","props":{"text":"x"},"children":["h"]}}}`)).toMatch(/^\/elements\/h\/children: Heading cannot have children/)
  expect(err(`{"root":"r","elements":{"r":{"type":"Separator"}},"programs":{}}`)).toMatch(/^\/programs: /)
  expect(err(`not json at all ][`)).toMatch(/^: invalid JSON after repair: /)
})

test('element limit', () => {
  const elements = Object.fromEntries(Array.from({ length: 300 }, (_, i) => [`e${i}`, { type: 'Separator' }]))
  elements.r = { type: 'Stack', children: Object.keys(elements) } as never
  expect(err(JSON.stringify({ root: 'r', elements }))).toMatch(/^\/: 301 elements, limit 256/)
})

test('cross-field rules: Heatmap grid shape, Image src scheme, LinkList url scheme', () => {
  const heat = (cells: string) => `{"root":"h","elements":{"h":{"type":"Heatmap","props":{"rows":["a","b"],"cols":["x","y","z"],"cells":${cells}}}}}`
  expect(ok(heat('[[1,2,3],[4,5,6]]')).spec.elements.h?.type).toBe('Heatmap')
  expect(err(heat('[[1,2,3]]'))).toBe('/elements/h/props/cells: 1 rows of cells, 2 rows')
  expect(err(heat('[[1,2,3],[4,5]]'))).toBe('/elements/h/props/cells/1: 2 cells, 3 columns')
  const img = (src: string) => `{"root":"i","elements":{"i":{"type":"Image","props":{"src":"${src}","alt":"a"}}}}`
  expect(ok(img('https://x.test/a.png')).repaired).toBe(false)
  expect(ok(img('data:image/png;base64,AA==')).repaired).toBe(false)
  expect(err(img('file:///etc/passwd'))).toBe('/elements/i/props/src: src must be http(s) or data:image')
  const links = (url: string) => `{"root":"l","elements":{"l":{"type":"LinkList","props":{"items":[{"title":"t","url":"${url}"}]}}}}`
  expect(ok(links('http://x.test')).repaired).toBe(false)
  expect(err(links('javascript:alert(1)'))).toBe('/elements/l/props/items/0/url: url must be http(s)')
})

test('Sequence and Excerpt: the broken fixtures fail at the Rust paths with the Rust messages; the valid ones parse', () => {
  expect(ok(fixture('valid-sequence')).spec.elements.s?.type).toBe('Sequence')
  expect(ok(fixture('valid-excerpt')).spec.elements.x?.type).toBe('Excerpt')
  expect(err(fixture('broken-excerpt-mark'))).toMatch(/^\/elements\/x\/props\/mark\/1: /)
  expect(err(fixture('broken-sequence-actor'))).toBe('/elements/s/props/steps/1/to: unknown actor "validator"')
  expect(err(fixture('broken-ref-scheme'))).toBe('/elements/g/props/nodes/0/ref: path must not carry a scheme: http:')
  const seq = (steps: string, actors = '[{"id":"a","label":"A"},{"id":"b","label":"B"}]') =>
    `{"root":"s","elements":{"s":{"type":"Sequence","props":{"actors":${actors},"steps":${steps}}}}}`
  expect(err(seq('[]'))).toBe('/elements/s/props/steps: 0 steps, expected 1..=40')
  expect(err(seq('[{"from":"a","to":"b","label":"x"}]', '[{"id":"a","label":"A"}]'))).toBe('/elements/s/props/actors: 1 actors, expected 2..=12')
  expect(err(seq('[{"from":"a","to":"b","label":"x"}]', '[{"id":"a","label":"A"},{"id":"a","label":"B"}]'))).toBe('/elements/s/props/actors/1/id: duplicate actor id "a"')
  expect(err(seq('[{"from":"a","to":"b","label":"x","ref":"../x"}]'))).toBe('/elements/s/props/steps/0/ref: path must not contain "..": "../x"')
  expect(err(seq('[{"from":"a","to":"b","label":"x","ref":"a#L9-L3"}]'))).toBe('/elements/s/props/steps/0/ref: L3 is before L9')
  const ex = (props: string) => `{"root":"x","elements":{"x":{"type":"Excerpt","props":${props}}}}`
  expect(err(ex('{"path":"/etc/passwd","text":"a"}'))).toBe('/elements/x/props/path: path must be root-relative, not absolute: "/etc/passwd"')
  expect(err(ex('{"path":"a.rs","text":""}'))).toBe('/elements/x/props/text: empty')
  expect(err(ex(`{"path":"a.rs","text":${JSON.stringify(Array(121).fill('x').join('\n'))}}`))).toBe('/elements/x/props/text: 121 lines, limit 120')
  expect(err(ex(`{"path":"a.rs","text":${JSON.stringify('é'.repeat(9000))}}`))).toBe('/elements/x/props/text: 18000 bytes, limit 16384')
  expect(err(ex('{"path":"a.rs","text":"a\\nb","start":5,"mark":[[6,5]]}'))).toBe('/elements/x/props/mark/0: [6, 5] is not ordered')
  expect(err(ex('{"path":"a.rs","text":"a\\nb","start":5,"mark":[[5,6],[4,5]]}'))).toBe('/elements/x/props/mark/1: [4, 5] is outside the quoted lines 5-6')
  expect(err(ex('{"path":"a.rs","text":"a","mark":[[1]]}'))).toMatch(/^\/elements\/x\/props\/mark\/0: /)
  expect(err(ex('{"path":"a.rs","text":"a","start":0}'))).toMatch(/^\/elements\/x\/props\/start: /)
  expect(excerptLines('a\r\nb\n')).toEqual(['a', 'b'])
  expect(excerptLines('a\n\n')).toEqual(['a', ''])
})

test('parsePartial renders the finished elements and drops the half-written one', () => {
  const spec = parsePartial(CARD.slice(0, CARD.indexOf('"trend"') + 4))
  expect(spec?.root).toBe('r')
  expect(Object.keys(spec!.elements).sort()).toEqual(['r'])
  expect(spec!.elements.r?.children).toEqual([])
  const later = parsePartial(CARD.slice(0, CARD.indexOf('"t":{')))
  expect(Object.keys(later!.elements).sort()).toEqual(['m', 'r'])
  expect(later!.elements.r?.children).toEqual(['m'])
  expect(parsePartial(CARD)).toEqual(ok(CARD).spec)
})

test('parsePartial is null until a root element exists and never throws', () => {
  expect(parsePartial('{')).toBeNull()
  expect(parsePartial('{"root":"r","elements":{"r":{"type":"Ca')).toBeNull()
  expect(parsePartial('')).toBeNull()
  expect(parsePartial('{"root":"a","elements":{"a":{"type":"Card","children":["a"]}}}')).toBeNull()
})

test('Deck interactive components (ActionButton, StreamControls, ResultBanner) mirror Rust validate.rs rules (H13)', () => {
  const btn = (props: Record<string, unknown>) =>
    JSON.stringify({ root: 'b', elements: { b: { type: 'ActionButton', props } } })
  expect(ok(btn({ label: 'Triage', action: 'triage', args_json: '{"severity":"P1"}' })).spec.elements.b?.type).toBe(
    'ActionButton',
  )
  expect(ok(btn({ label: 'Refresh', action: 'refresh' })).repaired).toBe(false)
  expect(err(btn({ label: 'Bad', action: '   ' }))).toBe('/elements/b/props/action: action must not be empty')
  expect(err(btn({ label: 'Bad', action: 'go', args_json: '{broken' }))).toBe(
    '/elements/b/props/args_json: args_json must be a JSON object',
  )
  expect(err(btn({ label: 'Bad', action: 'go', args_json: '' }))).toBe(
    '/elements/b/props/args_json: args_json must be a JSON object',
  )
  expect(err(btn({ label: 'Bad', action: 'go', args_json: '   ' }))).toBe(
    '/elements/b/props/args_json: args_json must be a JSON object',
  )
  expect(err(btn({ label: 'Bad', action: 'go', args_json: '[1,2]' }))).toBe(
    '/elements/b/props/args_json: args_json must be a JSON object',
  )
  expect(err(btn({ label: 'Bad', action: 'go', args_json: '"str"' }))).toBe(
    '/elements/b/props/args_json: args_json must be a JSON object',
  )
  expect(
    ok(JSON.stringify({ root: 's', elements: { s: { type: 'StreamControls', props: { show_run_once: true } } } })).spec
      .elements.s?.type,
  ).toBe('StreamControls')
  expect(
    ok(JSON.stringify({ root: 'r', elements: { r: { type: 'ResultBanner', props: { title: 'Out' } } } })).spec.elements
      .r?.type,
  ).toBe('ResultBanner')
})
