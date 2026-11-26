// Live dictation into an editable box, the way Apple's keyboard dictation works: every utterance lands at the caret,
// the box stays editable around it, and the utterance still being heard (the volatile span) shows at its insertion
// point, locked until the engine closes it. Pure: the composer feeds it the engine's utterances and the user's edits
// and renders `display()`.
//
// Coordinates. `base` is the box without the volatile span; the span sits at `vol.anchor` in it. `sel` and the user's
// edits are in display coordinates (what the textarea holds); segments and undo steps in base coordinates, so the
// undo stack replays from `origin` to `base` once the dictation ends (`replay`).

import { type EditDoc, type EditableText, editText } from '@/lib/text-edit'

export type Sel = { start: number; end: number }
/** The utterance still being heard: `text` = lead space + body + trailing space; `caret` = lead + body length. */
export type Volatile = {
  utt: number
  anchor: number
  text: string
  caret: number
  /** The caret rides at the end of the body; a click, an arrow or a keystroke lets go. */
  follow: boolean
  /** The selection the span replaced (taken out by an `open` step). */
  replaced: string
}
/** A closed utterance as it landed, tracked while intact so a later final for it ("scratch that") can rewrite it. */
export type Seg = { utt: number; start: number; text: string }
/** One undo step in base coordinates; `open` marks the removal of the selection a span replaced. */
export type Step = { start: number; removed: string; inserted: string; before: Sel; after: Sel; typed?: boolean; open?: number }
export type DictBuffer = { origin: string; base: string; sel: Sel; vol: Volatile | null; segs: Seg[]; undo: Step[]; redo: Step[] }

const clamp = (n: number, max: number) => Math.max(0, Math.min(n, max))

export function begin(text: string, start: number, end: number): DictBuffer {
  const s = clamp(Math.min(start, end), text.length)
  return { origin: text, base: text, sel: { start: s, end: clamp(Math.max(start, end), text.length) }, vol: null, segs: [], undo: [], redo: [] }
}

/** What the box shows: `base` with the volatile span at its anchor. */
export function display(b: DictBuffer): string {
  return b.vol ? b.base.slice(0, b.vol.anchor) + b.vol.text + b.base.slice(b.vol.anchor) : b.base
}

/** The volatile span in display coordinates, `null` when nothing is being heard. */
export function volatileRange(b: DictBuffer): Sel | null {
  return b.vol && b.vol.text ? { start: b.vol.anchor, end: b.vol.anchor + b.vol.text.length } : null
}

/** Display → base; a position inside the span maps to its anchor. */
function toBase(b: DictBuffer, p: number): number {
  if (!b.vol) return p
  const { anchor: a, text } = b.vol
  return p <= a ? p : p >= a + text.length ? p - text.length : a
}
/** Base → display; a position at the anchor lands before the span unless `after`. */
function toDisplay(b: DictBuffer, p: number, after = false): number {
  if (!b.vol) return p
  const a = b.vol.anchor
  return p < a || (p === a && !after) ? p : p + b.vol.text.length
}
const selToBase = (b: DictBuffer, s: Sel): Sel => ({ start: toBase(b, s.start), end: toBase(b, s.end) })
const selToDisplay = (b: DictBuffer, s: Sel): Sel => ({ start: toDisplay(b, s.start), end: toDisplay(b, s.end) })

/**
 * Replace base `[start, end)` with `inserted`, carrying the span's anchor and the tracked segments along. A collapsed
 * edit at the anchor goes after the span when `after` (the user typed right behind it), else before it.
 */
function replaceBase(b: DictBuffer, start: number, end: number, inserted: string, after = false): DictBuffer {
  const delta = inserted.length - (end - start)
  const base = b.base.slice(0, start) + inserted + b.base.slice(end)
  let vol = b.vol
  if (vol) {
    const a = vol.anchor
    if (end < a || (end === a && !(after && start === end))) vol = { ...vol, anchor: a + delta }
    else if (start < a) vol = { ...vol, anchor: start + inserted.length }
  }
  const segs: Seg[] = []
  for (const g of b.segs) {
    if (end <= g.start) segs.push(delta ? { ...g, start: g.start + delta } : g)
    else if (start >= g.start + g.text.length) segs.push(g)
    // An edit into the segment: it is the user's text now.
  }
  return { ...b, base, vol, segs }
}

/** Shift a base-coordinate position for an edit already applied at `[start, end)` → `inserted`. */
function shift(p: number, start: number, end: number, inserted: string): number {
  if (p >= end) return p + inserted.length - (end - start)
  if (p > start) return start + inserted.length
  return p
}

/**
 * Whether an `InputEvent` of `inputType` on display selection `[start, end)` would change the volatile span. The
 * composer cancels those in `beforeinput`; `edit` refuses whatever still gets through (an IME, a word deletion).
 */
export function locked(b: DictBuffer, start: number, end: number, inputType: string): boolean {
  const r = volatileRange(b)
  if (!r) return false
  if (start !== end) return start < r.end && end > r.start
  if (/^delete.*Backward$/.test(inputType)) return start > r.start && start <= r.end
  if (/^delete.*Forward$/.test(inputType)) return start >= r.start && start < r.end
  return start > r.start && start < r.end
}

/**
 * The single replacement an `input` made: `prev` → `next` with the caret at `caret` after it. The text after the caret
 * is what survived, so a repeated letter is placed where it was typed (a plain diff would guess).
 */
export function inputEdit(prev: string, next: string, caret: number): { start: number; end: number; text: string } | null {
  if (prev === next) return null
  const keep = next.length - caret
  const end = prev.length - keep
  if (keep >= 0 && end >= 0 && prev.slice(end) === next.slice(caret)) {
    let start = 0
    const max = Math.min(caret, end)
    while (start < max && prev.charCodeAt(start) === next.charCodeAt(start)) start++
    return { start, end, text: next.slice(start, caret) }
  }
  let start = 0
  while (start < Math.min(prev.length, next.length) && prev[start] === next[start]) start++
  let pe = prev.length
  let ne = next.length
  while (pe > start && ne > start && prev[pe - 1] === next[ne - 1]) {
    pe--
    ne--
  }
  return { start, end: pe, text: next.slice(start, ne) }
}

/** Push a step, clearing redo; consecutive typed letters within a word make one step, as the native stack does. */
function push(b: DictBuffer, step: Step): DictBuffer {
  const last = b.undo[b.undo.length - 1]
  if (
    step.typed &&
    last?.typed &&
    !last.removed &&
    !step.removed &&
    step.start === last.start + last.inserted.length &&
    !/\s$/.test(last.inserted)
  ) {
    return { ...b, undo: [...b.undo.slice(0, -1), { ...last, inserted: last.inserted + step.inserted, after: step.after }], redo: [] }
  }
  return { ...b, undo: [...b.undo, step], redo: [] }
}

/**
 * The user replaced display `[start, end)` with `inserted` (typing, paste, deletion), leaving `after` selected.
 * `null` when it touches the volatile span: the caller puts the box back to `display(b)`.
 */
export function edit(b: DictBuffer, start: number, end: number, inserted: string, after: Sel): DictBuffer | null {
  let s = start
  let e = end
  let behind = false
  if (b.vol) {
    const a = b.vol.anchor
    const z = a + b.vol.text.length
    if (end <= a) {
      // before the span (a collapsed edit at the anchor goes before it)
    } else if (start >= z) {
      s -= b.vol.text.length
      e -= b.vol.text.length
      behind = true
    } else return null
  }
  const removed = b.base.slice(s, e)
  const before = selToBase(b, b.sel)
  let next = replaceBase(b, s, e, inserted, behind)
  next = { ...next, sel: after, vol: next.vol && { ...next.vol, follow: false } }
  return push(next, { start: s, removed, inserted, before, after: selToBase(next, after), typed: true })
}

/** The user moved the caret or the selection (display coordinates). The span stops following unless it is where it was put. */
export function select(b: DictBuffer, start: number, end: number): DictBuffer {
  if (b.sel.start === start && b.sel.end === end) return b
  const v = b.vol
  const at = v ? v.anchor + v.caret : -1
  const vol = v && v.follow && !(start === at && end === at) ? { ...v, follow: false } : v
  return { ...b, sel: { start, end }, vol }
}

/**
 * Fit `raw` between `before` and `after`: a space where it would touch a word, and a capital after a sentence end (or
 * at the start) when the first word is all lower case (never "iPhone"). Nothing is lowered mid-sentence.
 */
export function decorate(before: string, after: string, raw: string): { text: string; caret: number } {
  if (!raw) return { text: '', caret: 0 }
  let body = raw
  if (/(?:^|[.!?\n])\s*$/.test(before) && /^\p{Ll}[\p{Ll}']*(?![\p{L}\d])/u.test(body)) body = body[0]!.toUpperCase() + body.slice(1)
  const lead = before && !/[\s([{]$/.test(before) && !/^[\s.,;:!?)\]}]/.test(body) ? ' ' : ''
  const trail = after && !/^[\s.,;:!?)\]}]/.test(after) && !/\s$/.test(body) ? ' ' : ''
  return { text: lead + body + trail, caret: lead.length + body.length }
}

/** Where the selection goes when the span changes from `prev` to `next` while the user holds the caret elsewhere. */
function carry(prev: DictBuffer, next: DictBuffer): Sel {
  const map = (p: number) => {
    const v = prev.vol
    if (v && p > v.anchor && p < v.anchor + v.text.length) return next.vol ? next.vol.anchor + Math.min(p - v.anchor, next.vol.text.length) : toDisplay(next, v.anchor)
    return toDisplay(next, toBase(prev, p), v !== null && p >= v.anchor + v.text.length && p > v.anchor)
  }
  return { start: map(prev.sel.start), end: map(prev.sel.end) }
}

/** Set the span's words; the caret follows when it was left there. */
function setVol(b: DictBuffer, raw: string): DictBuffer {
  const v = b.vol
  if (!v) return b
  const d = decorate(b.base.slice(0, v.anchor), b.base.slice(v.anchor), raw)
  const next: DictBuffer = { ...b, vol: { ...v, text: d.text, caret: d.caret } }
  const at = v.anchor + d.caret
  return { ...next, sel: v.follow ? { start: at, end: at } : carry(b, next) }
}

/** Open a span at the caret, taking out the selection first (one `open` step, merged into the segment's on commit). */
function open(b: DictBuffer, utt: number): DictBuffer {
  const sel = selToBase(b, b.sel)
  let next = b
  let replaced = ''
  if (sel.end > sel.start) {
    replaced = b.base.slice(sel.start, sel.end)
    next = replaceBase(b, sel.start, sel.end, '')
    const at = { start: sel.start, end: sel.start }
    next = push({ ...next, sel: at }, { start: sel.start, removed: replaced, inserted: '', before: sel, after: at, open: utt })
  }
  return { ...next, sel: { start: sel.start, end: sel.start }, vol: { utt, anchor: sel.start, text: '', caret: 0, follow: true, replaced } }
}

/** Close the span as it stands: its words become ordinary text, one undo step (with the selection it replaced). */
function commit(b: DictBuffer): DictBuffer {
  const v = b.vol
  if (!v) return b
  const a = v.anchor
  const last = b.undo[b.undo.length - 1]
  const merge = last?.open === v.utt && last.start === a
  if (!v.text) {
    // Nothing was said: the selection it took comes back.
    const bare = { ...b, vol: null }
    const sel = selToBase(b, b.sel)
    if (!merge || !last) return { ...bare, sel }
    const back = replaceBase(bare, a, a, last.removed)
    return { ...back, undo: b.undo.slice(0, -1), sel: last.before }
  }
  // The user's selection, in base coordinates once the span's words are part of it.
  const len = v.text.length
  const mapSel = (p: number) => (p <= a ? p : p >= a + len ? p : a + v.caret)
  const sel = v.follow ? { start: a + v.caret, end: a + v.caret } : { start: mapSel(b.sel.start), end: mapSel(b.sel.end) }
  let next = replaceBase({ ...b, vol: null }, a, a, v.text)
  next = { ...next, sel, segs: [...next.segs, { utt: v.utt, start: a, text: v.text }] }
  if (merge && last)
    return { ...next, undo: [...b.undo.slice(0, -1), { start: last.start, removed: last.removed, inserted: v.text, before: last.before, after: sel }], redo: [] }
  return push(next, { start: a, removed: '', inserted: v.text, before: { start: a, end: a }, after: sel })
}

/** A final for an utterance already landed ("scratch that" empties it, 153 §7.4): rewrite it where it is, if intact. */
function revise(b: DictBuffer, g: Seg, raw: string): DictBuffer {
  const end = g.start + g.text.length
  if (b.base.slice(g.start, end) !== g.text) return { ...b, segs: b.segs.filter((x) => x !== g) }
  const d = decorate(b.base.slice(0, g.start), b.base.slice(end), raw)
  if (d.text === g.text) return b
  const before = selToBase(b, b.sel)
  let next = replaceBase(b, g.start, end, d.text)
  const map = (p: number) => shift(p, g.start, end, d.text)
  const after = { start: map(before.start), end: map(before.end) }
  next = { ...next, sel: selToDisplay(next, after), segs: d.text ? [...next.segs, { utt: g.utt, start: g.start, text: d.text }] : next.segs }
  return push(next, { start: g.start, removed: g.text, inserted: d.text, before, after })
}

/** The engine's open words for utterance `utt`: the span shows them at its anchor, opening at the caret if new. */
export function partial(b: DictBuffer, utt: number, raw: string): DictBuffer {
  let next = b
  if (next.vol && next.vol.utt !== utt) {
    if (utt < next.vol.utt) return next
    next = commit(next)
  }
  if (!next.vol) {
    if (!raw || next.segs.some((g) => g.utt === utt)) return next
    next = open(next, utt)
  }
  return setVol(next, raw)
}

/** The engine closed utterance `utt` with `raw`: it lands at its anchor as ordinary text. */
export function final(b: DictBuffer, utt: number, raw: string): DictBuffer {
  if (b.vol?.utt === utt) return commit(setVol(b, raw))
  const g = b.segs.find((x) => x.utt === utt)
  if (g) return revise(b, g, raw)
  if (b.vol && utt < b.vol.utt) return b
  const next = b.vol ? commit(b) : b
  if (!raw) return next
  return commit(setVol(open(next, utt), raw))
}

function apply(b: DictBuffer, from: string, to: string, start: number): DictBuffer | null {
  if (b.base.slice(start, start + from.length) !== from) return null
  return replaceBase(b, start, start + from.length, to)
}

/** ⌘Z: take back the last step (a landed utterance, or the user's typing). A stack that no longer fits is dropped. */
export function undo(b: DictBuffer): DictBuffer {
  const step = b.undo[b.undo.length - 1]
  if (!step) return b
  const next = apply(b, step.inserted, step.removed, step.start)
  if (!next) return { ...b, undo: [], redo: [] }
  return { ...next, sel: selToDisplay(next, step.before), undo: b.undo.slice(0, -1), redo: [...b.redo, step] }
}

/** ⇧⌘Z: put the last undone step back. */
export function redo(b: DictBuffer): DictBuffer {
  const step = b.redo[b.redo.length - 1]
  if (!step) return b
  const next = apply(b, step.removed, step.inserted, step.start)
  if (!next) return { ...b, redo: [] }
  return { ...next, sel: selToDisplay(next, step.after), undo: [...b.undo, step], redo: b.redo.slice(0, -1) }
}

/**
 * The dictation ended. Kept: the span still open lands as it stands. `discard` (Esc): the span goes and every
 * utterance still intact is taken out; the user's own typing stays. `steps` turn `origin` into `value` (`replay`).
 */
export function end(b: DictBuffer, discard: boolean): { value: string; sel: Sel; origin: string; steps: Step[] } {
  let next = b
  if (discard) {
    if (next.vol) next = commit({ ...next, vol: { ...next.vol, text: '', caret: 0 } })
    for (const g of [...next.segs].sort((x, y) => y.start - x.start)) next = revise(next, g, '')
  } else next = commit(next)
  return { value: next.base, sel: next.sel, origin: next.origin, steps: next.undo }
}

/** `origin` with `steps` applied, in order: `end()`'s `value`. */
export function replay(origin: string, steps: Step[]): string {
  return steps.reduce((t, s) => t.slice(0, s.start) + s.inserted + t.slice(s.start + s.removed.length), origin)
}

/**
 * Land a finished dictation in the box as native editing commands, one per step from `origin`, so ⌘Z after the
 * dictation walks back utterance by utterance. `false` (caller writes the value instead) when a command did not take.
 */
export function replayInto(ta: EditableText, origin: string, steps: Step[], doc?: EditDoc): boolean {
  ta.value = origin
  for (const s of steps) if (!editText(ta, s.start, s.start + s.removed.length, s.inserted, doc)) return false
  return true
}
