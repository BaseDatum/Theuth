// 96 §6.6: the `[screen i/n …]` line the voice key appends to a user message, and the `[image: <name>]` /
// `[document: <name>]` tokens the engine renders binary parts as (17 §3.5, `agent::loop_::binary_tag`). Mirrors
// `computer::context::line`; a line the grammar does not admit stays body text.

export type Element = { role: string; label?: string; url?: string }
export type Gesture = {
  kind: 'circled' | 'underlined' | 'rested' | 'ended'
  from?: [number, number]
  to?: [number, number]
  at?: [number, number]
  ms?: number
  elements: Element[]
}
export type Screen = { index: number; count: number; app?: string; title?: string; url?: string; gestures: Gesture[]; accessibility: boolean }

/** `"<label>"`: the writer does not escape, so a quote inside a label is admitted by requiring the closing quote to be followed by a separator. */
const Q = '"((?:\\\\.|[^\\\\])*?)"'
const HEAD = new RegExp(`^\\[screen (\\d+)/(\\d+)(?: ${Q}(?= · ))?(?: · ${Q}(?= · ))?(?: · (\\S+?)(?= · ))?(?: · pointer: (.*?))? · accessibility: (on|off)\\]$`)
const PT = '\\((-?\\d+),(-?\\d+)\\)'
const CIRCLED = new RegExp(`^circled ${PT}–${PT}(?:: (.*))?$`)
const UNDERLINED = new RegExp(`^underlined ${PT}→${PT}(?:: (.*))?$`)
const RESTED = new RegExp(`^rested on ${PT} for (\\d+) ms(?:: (.*))?$`)
const ENDED_AT = new RegExp(`^ended on ${PT}$`)
/** One element at the cursor: `<role>[ "<label>"][ → <url>]`, each part ending at `, `, `; ` or the end. */
const ELEMENT = new RegExp(`([A-Za-z][\\w-]*)(?: ${Q}(?=$| → |, ))?(?: → (\\S+?)(?=$|, ))?`, 'y')
const TOKEN = / ?\[(image|document): ([^\]\n]+)\]/g

const unescape = (s: string) => s.replace(/\\(.)/g, '$1')
const pt = (x: string | undefined, y: string | undefined): [number, number] => [Number(x), Number(y)]

/** `, `-separated elements; a label may itself hold `, ` — the closing quote must be followed by a separator. */
export function parseElements(s: string): Element[] {
  const out: Element[] = []
  let i = 0
  while (i < s.length) {
    ELEMENT.lastIndex = i
    const m = ELEMENT.exec(s)
    if (!m || m[0] === '') return out
    const e: Element = { role: m[1] ?? '' }
    if (m[2] !== undefined) e.label = unescape(m[2])
    if (m[3] !== undefined) e.url = m[3]
    out.push(e)
    i = ELEMENT.lastIndex
    if (s.startsWith(', ', i)) i += 2
    else break
  }
  return out
}

/** One `; `-separated gesture, or `null` when it is not one the grammar admits. */
export function parseGesture(s: string): Gesture | null {
  let m = CIRCLED.exec(s)
  if (m) return { kind: 'circled', from: pt(m[1], m[2]), to: pt(m[3], m[4]), elements: parseElements(m[5] ?? '') }
  m = UNDERLINED.exec(s)
  if (m) return { kind: 'underlined', from: pt(m[1], m[2]), to: pt(m[3], m[4]), elements: parseElements(m[5] ?? '') }
  m = RESTED.exec(s)
  if (m) return { kind: 'rested', at: pt(m[1], m[2]), ms: Number(m[3]), elements: parseElements(m[4] ?? '') }
  m = ENDED_AT.exec(s)
  if (m) return { kind: 'ended', at: pt(m[1], m[2]), elements: [] }
  if (s.startsWith('ended on ')) return { kind: 'ended', elements: parseElements(s.slice('ended on '.length)) }
  return null
}

/** Gestures are `; `-separated; a `; ` inside a quoted label is not a separator. */
function splitGestures(s: string): string[] {
  const out: string[] = []
  let start = 0
  let quoted = false
  for (let i = 0; i < s.length; i++) {
    const c = s[i]
    if (c === '\\' && quoted) i++
    else if (c === '"') quoted = !quoted
    else if (!quoted && c === ';' && s[i + 1] === ' ') {
      out.push(s.slice(start, i))
      start = i + 2
    }
  }
  out.push(s.slice(start))
  return out
}

/** One `[screen …]` line as a `Screen`; `null` when it is not one. */
export function parseScreenLine(line: string): Screen | null {
  const m = HEAD.exec(line)
  if (!m) return null
  const gestures: Gesture[] = []
  for (const g of m[6] ? splitGestures(m[6]) : []) {
    const parsed = parseGesture(g)
    if (!parsed) return null
    gestures.push(parsed)
  }
  const s: Screen = { index: Number(m[1]), count: Number(m[2]), gestures, accessibility: m[7] === 'on' }
  if (m[3] !== undefined) s.app = unescape(m[3])
  if (m[4] !== undefined) s.title = unescape(m[4])
  if (m[5] !== undefined) s.url = m[5]
  return s
}

/**
 * Split a user body into its words, its `[screen …]` lines and its `[image: …]`/`[document: …]` file names (in
 * order, so `images` is overviews then crops for a voice-key message). Tokens may be glued to a line's end; a
 * `[screen …]` line is one whole line. Anything the grammar does not admit stays in `body`.
 */
export function parseScreenLines(text: string): { body: string; screens: Screen[]; images: string[] } {
  const images: string[] = []
  const stripped = text.replace(TOKEN, (_, _kind, name: string) => {
    images.push(name)
    return ''
  })
  const screens: Screen[] = []
  const body: string[] = []
  for (const line of stripped.split('\n')) {
    const s = parseScreenLine(line)
    if (s) screens.push(s)
    else body.push(line)
  }
  return { body: body.join('\n').trim(), screens, images }
}
