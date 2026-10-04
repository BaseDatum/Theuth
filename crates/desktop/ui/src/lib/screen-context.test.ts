import { expect, test } from 'vitest'
import { parseElements, parseGesture, parseScreenLine, parseScreenLines } from './screen-context'

const WIN = 'window "majiayu000/litellm-rs: Self-hosted Rust LLM gateway with OpenAI-compatible APIs,…"'
const SKY = `Can you clone this repo\n[screen 1/1 "theuth" · pointer: circled (153,91)–(831,355): ${WIN}; underlined (630,359)→(714,360): ${WIN}; rested on (711,358) for 1849 ms: ${WIN}; ended on ${WIN} · accessibility: on][image: eec889cc446240df.png][image: 31d922047487392b.png][image: 53cedad67f01b3e3.png][image: f330ce422f86eeb0.png]`

const DOC =
  '[screen 1/1 "Google Chrome" · "basedatum — GitHub" · https://github.com/basedatum?tab=repositories · pointer: circled (612,418)–(884,452): link "theuth" → https://github.com/basedatum/theuth, text "Rust · Updated 2 hours ago"; ended on link "theuth" → https://github.com/basedatum/theuth · accessibility: on]'

test("Sky's line: the body, four gestures each on one window element, four glued image tokens", () => {
  const { body, screens, images } = parseScreenLines(SKY)
  expect(body).toBe('Can you clone this repo')
  expect(images).toEqual(['eec889cc446240df.png', '31d922047487392b.png', '53cedad67f01b3e3.png', 'f330ce422f86eeb0.png'])
  expect(screens).toHaveLength(1)
  const s = screens[0]!
  expect(s).toMatchObject({ index: 1, count: 1, app: 'theuth', accessibility: true })
  expect(s.title).toBeUndefined()
  expect(s.url).toBeUndefined()
  expect(s.gestures.map((g) => g.kind)).toEqual(['circled', 'underlined', 'rested', 'ended'])
  const el = { role: 'window', label: 'majiayu000/litellm-rs: Self-hosted Rust LLM gateway with OpenAI-compatible APIs,…' }
  expect(s.gestures[0]).toEqual({ kind: 'circled', from: [153, 91], to: [831, 355], elements: [el] })
  expect(s.gestures[1]).toEqual({ kind: 'underlined', from: [630, 359], to: [714, 360], elements: [el] })
  expect(s.gestures[2]).toEqual({ kind: 'rested', at: [711, 358], ms: 1849, elements: [el] })
  expect(s.gestures[3]).toEqual({ kind: 'ended', elements: [el] })
})

test('§6.6 example: app, title, url; link + text elements with a `·` and a comma-free label; ended on a link', () => {
  const s = parseScreenLine(DOC)!
  expect(s).toMatchObject({ index: 1, count: 1, app: 'Google Chrome', title: 'basedatum — GitHub', url: 'https://github.com/basedatum?tab=repositories', accessibility: true })
  expect(s.gestures).toEqual([
    {
      kind: 'circled',
      from: [612, 418],
      to: [884, 452],
      elements: [
        { role: 'link', label: 'theuth', url: 'https://github.com/basedatum/theuth' },
        { role: 'text', label: 'Rust · Updated 2 hours ago' },
      ],
    },
    { kind: 'ended', elements: [{ role: 'link', label: 'theuth', url: 'https://github.com/basedatum/theuth' }] },
  ])
})

test('header with only the app; header with nothing; a 2/2 badge count', () => {
  expect(parseScreenLine('[screen 1/1 "Finder" · pointer: ended on (10,20) · accessibility: off]')).toEqual({
    index: 1,
    count: 1,
    app: 'Finder',
    gestures: [{ kind: 'ended', at: [10, 20], elements: [] }],
    accessibility: false,
  })
  // `computer::context` tests' second display: no front app, elements absent everywhere.
  const s = parseScreenLine('[screen 2/2 · pointer: underlined (100,500)→(400,500); rested on (488,300) for 640 ms; ended on (488,300) · accessibility: off]')!
  expect(s).toMatchObject({ index: 2, count: 2, accessibility: false })
  expect(s.app).toBeUndefined()
  expect(s.gestures).toEqual([
    { kind: 'underlined', from: [100, 500], to: [400, 500], elements: [] },
    { kind: 'rested', at: [488, 300], ms: 640, elements: [] },
    { kind: 'ended', at: [488, 300], elements: [] },
  ])
})

test('a gesture with no element list; no pointer field at all', () => {
  expect(parseGesture('circled (1,2)–(3,4)')).toEqual({ kind: 'circled', from: [1, 2], to: [3, 4], elements: [] })
  expect(parseScreenLine('[screen 1/1 "App" · accessibility: on]')).toMatchObject({ app: 'App', gestures: [], accessibility: true })
})

test('elements: commas and `; ` inside labels, escaped and raw quotes, a role without a label, a url without a label', () => {
  expect(parseElements('text "a, b; c", button "say \\"hi\\"", field "it\'s "quoted" here", button, link → https://x.test/')).toEqual([
    { role: 'text', label: 'a, b; c' },
    { role: 'button', label: 'say "hi"' },
    { role: 'field', label: 'it\'s "quoted" here' },
    { role: 'button' },
    { role: 'link', url: 'https://x.test/' },
  ])
  const s = parseScreenLine('[screen 1/1 "A" · pointer: circled (0,0)–(1,1): text "x; y"; ended on text "z" · accessibility: on]')!
  expect(s.gestures.map((g) => g.kind)).toEqual(['circled', 'ended'])
  expect(s.gestures[0]?.elements).toEqual([{ role: 'text', label: 'x; y' }])
})

test('a title that holds ` · ` and a url with a query string', () => {
  const s = parseScreenLine('[screen 1/1 "Safari" · "Rust · Updated · GitHub" · https://a.test/b?c=d&e=f · pointer: ended on (1,1) · accessibility: on]')!
  expect(s.title).toBe('Rust · Updated · GitHub')
  expect(s.url).toBe('https://a.test/b?c=d&e=f')
})

test('image and document tokens without a screen line: thumbnails only, body kept', () => {
  expect(parseScreenLines('look at this [image: ab.png] and [document: cv.pdf]')).toEqual({ body: 'look at this and', screens: [], images: ['ab.png', 'cv.pdf'] })
  expect(parseScreenLines('[image: a.png][image: b.png]')).toEqual({ body: '', screens: [], images: ['a.png', 'b.png'] })
})

test('text that is not a screen line stays body; a malformed screen line stays body; two displays in order', () => {
  expect(parseScreenLines('[screen time] is over\nplain')).toEqual({ body: '[screen time] is over\nplain', screens: [], images: [] })
  const bad = '[screen 1/1 "A" · pointer: waved (1,2) · accessibility: on]'
  expect(parseScreenLines(bad).body).toBe(bad)
  const two = `go\n[screen 1/2 "A" · pointer: ended on (1,1) · accessibility: on]\n[screen 2/2 · pointer: ended on (2,2) · accessibility: on]`
  const r = parseScreenLines(two)
  expect(r.body).toBe('go')
  expect(r.screens.map((s) => [s.index, s.count])).toEqual([
    [1, 2],
    [2, 2],
  ])
})
