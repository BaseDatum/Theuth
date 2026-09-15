import { expect, test } from 'vitest'
import { parsePartial, parseSpec } from './parse'

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
