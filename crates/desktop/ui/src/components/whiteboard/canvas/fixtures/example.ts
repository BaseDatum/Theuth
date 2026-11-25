// The contract's §2.10 worked example, written into a Y.Doc with the exact shapes of §2 (nested maps for cards, links,
// frames, marks; a nested Y.Array for versions; plain values for strokes). `canvas/model.test.ts` encodes it to
// `example-v1.update.b64`, which B1's `reads_the_yjs_fixture` reads on the Rust side.
import * as Y from 'yjs'

export const EXAMPLE_PTS_HEX = '000000008000' + '0c000200ff02' + 'f0fff6ff3303'
export const hexBytes = (hex: string) => Uint8Array.from(hex.match(/../g)!.map((b) => parseInt(b, 16)))

const SESSION = 'session:01a20fc8-92a6-73e6-a622-3ac9e51ae923'
const SID = '01a20fc8-92a6-73e6-a622-3ac9e51ae923'

function nested(fields: Record<string, unknown>): Y.Map<unknown> {
  const m = new Y.Map<unknown>()
  for (const [k, v] of Object.entries(fields)) m.set(k, v)
  return m
}

/** Writes the example into `doc` (one transaction). */
export function writeExample(doc: Y.Doc): void {
  doc.transact(() => {
    const meta = doc.getMap<unknown>('meta')
    meta.set('v', 1)
    meta.set('created', 1795570000000)
    meta.set('bg', 'dots')

    const cards = doc.getMap<unknown>('cards')
    const v1 = new Y.Array<unknown>()
    const page = nested({
      kind: 'page', x: 250, y: 80, w: 180, h: 390, z: 2, title: 'Mock B — flat list',
      viewport: { w: 390, h: 844, preset: 'phone' }, state: 'live', frame: '01KAR3ZC4M0000000000000009',
      snap: { v: 2, at: 1795570405000, theme: 'light' }, by: SESSION,
    })
    page.set('versions', v1)
    cards.set('01KAR3ZC4M0000000000000001', page)
    v1.push([
      { source: { t: 'page', session: SID, call_id: 'toolu_01AAA' }, at: 1795570100000, by: SESSION },
      { source: { t: 'page', session: SID, call_id: 'toolu_01BBB' }, at: 1795570400000, by: SESSION, note: "A's header, filter chips" },
    ])
    const v2 = new Y.Array<unknown>()
    const sticky = nested({ kind: 'sticky', x: 50, y: 494, w: 220, h: 150, z: 1, title: 'Requirements', state: 'live', by: 'owner' })
    sticky.set('versions', v2)
    cards.set('01KAR3ZC4M0000000000000002', sticky)
    v2.push([{ source: { t: 'sticky', text: '- Triage with one thumb\n- Never lose a decision' }, at: 1795570050000, by: 'owner' }])

    doc.getMap<unknown>('links').set('01KAR3ZC4M0000000000000005', nested({
      from: '01KAR3ZC4M0000000000000002', to: '01KAR3ZC4M0000000000000001', label: 'drives', style: 'solid', arrow: 'end', by: 'owner',
    }))
    doc.getMap<unknown>('frames').set('01KAR3ZC4M0000000000000009', nested({
      title: 'Variants', x: 36, y: 48, w: 610, h: 424, colour: 'neutral', preset: 'phone', by: SESSION,
    }))
    doc.getMap<unknown>('ink').set('01KAR3ZC4M0000000000000007', {
      pts: hexBytes(EXAMPLE_PTS_HEX), n: 3, o: [100, 50], bb: [98.1, 47.6, 4.8, 4.05], tool: 'pen', colour: 'red', width: 2.8, p: 1,
      card: '01KAR3ZC4M0000000000000001', ver: 1, cs: [180, 390], at: 1795570200000, by: 'owner',
    })
    doc.getMap<unknown>('marks').set('01KAR3ZC4M0000000000000008', nested({
      strokes: ['01KAR3ZC4M0000000000000007'], card: '01KAR3ZC4M0000000000000001', ver: 1, bb: [98.1, 47.6, 4.8, 4.05], state: 'answered',
      region: { kind: 'page', label: 'tab bar, item 3', role: 'tab' }, sent_at: 1795570300000, to: [SID], blob: true, by: 'owner',
    }))
    const aliases = doc.getMap<unknown>('aliases')
    aliases.set('01KAR3ZC4M0000000000000001', 'c3')
    aliases.set('01KAR3ZC4M0000000000000002', 'c4')
    aliases.set('01KAR3ZC4M0000000000000005', 'l1')
    aliases.set('01KAR3ZC4M0000000000000009', 'f1')
    aliases.set('01KAR3ZC4M0000000000000008', 'm4')
  })
}

/** A doc with a fixed client id, so its encoding is byte-stable. */
export function exampleDoc(): Y.Doc {
  const doc = new Y.Doc()
  doc.clientID = 1
  writeExample(doc)
  return doc
}
