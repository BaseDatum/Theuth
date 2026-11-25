// 218 P5 (cheap parts): a whiteboard → a note brief (the outline in reading order, every card a
// `theuth://whiteboards/<id>#<card>` link), and the current view as a PNG (native capture; the desktop and iPad apps).

import type { Board, Card, Id } from './canvas/model'
import { versionOf } from './canvas/model'
import { rowsOf } from './canvas/rows'
import { KIND_LABEL } from './OutlineView'

export const cardHref = (doc: string, card: Id) => `theuth://whiteboards/${doc}#${card}`

const esc = (s: string) => s.replace(/([[\]\\])/g, '\\$1')

function cardLine(doc: string, c: Card): string {
  const parts = [`[${esc(c.title || 'Untitled')}](${cardHref(doc, c.id)})`, KIND_LABEL[c.kind]]
  if (c.state === 'dimmed') parts.push(c.tag ? `set aside — ${c.tag}` : 'set aside')
  const v = versionOf(c)
  if (v?.note) parts.push(v.note)
  let line = `- ${parts.join(' · ')}`
  const src = v?.source
  if (src?.t === 'sticky' && src.text.trim()) line += `\n${src.text.trim().split('\n').map((l) => `  > ${l}`).join('\n')}`
  return line
}

/** The brief: `# title`, one section per frame (then the loose cards), links, and the pinned marks. */
export function briefMarkdown(board: Board, title: string, doc: string): string {
  const { frames, loose } = rowsOf(board)
  const out: string[] = [`# ${title || 'Whiteboard'}`, '', `From [the whiteboard](theuth://whiteboards/${doc}).`]
  const section = (heading: string | null, rows: Id[][]) => {
    const cards = rows.flat().flatMap((id) => (board.cards[id] ? [board.cards[id] as Card] : []))
    if (!cards.length) return
    out.push('')
    if (heading) out.push(`## ${heading}`, '')
    for (const c of cards) out.push(cardLine(doc, c))
  }
  for (const { frame, rows } of frames) section(frame.title || 'Frame', rows)
  section(frames.length ? 'Outside frames' : null, loose)
  const live = (id: Id) => {
    const c = board.cards[id]
    return c !== undefined && c.state !== 'archived'
  }
  const links = Object.values(board.links).filter((l) => live(l.from) && live(l.to))
  if (links.length) {
    out.push('', '## Links', '')
    for (const l of links) out.push(`- ${board.cards[l.from]?.title || 'Untitled'} → ${board.cards[l.to]?.title || 'Untitled'}${l.label ? ` (${l.label})` : ''}`)
  }
  const pinned = Object.values(board.marks).filter((m) => m.state === 'pinned')
  if (pinned.length) {
    out.push('', '## Pinned marks', '')
    for (const m of pinned) {
      const on = m.card ? board.cards[m.card]?.title || 'a card' : 'the canvas'
      const what = [m.region?.label && `page says: ${m.region.label}`, m.hw && `handwriting: ${m.hw}`].filter(Boolean).join(' · ')
      out.push(`- on ${on}${what ? ` — ${what}` : ''}`)
    }
  }
  return `${out.join('\n')}\n`
}
