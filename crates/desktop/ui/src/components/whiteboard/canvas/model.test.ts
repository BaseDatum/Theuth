import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { EXAMPLE_PTS_HEX, exampleDoc, hexBytes } from './fixtures/example'
import { lifeOf, readBoard } from './model'

const FIXTURE = fileURLToPath(new URL('./fixtures/example-v1.update.b64', import.meta.url))

describe('the §2.10 example through a Y.Doc and back', () => {
  it('reads back exactly the contract shapes', () => {
    const b = readBoard(exampleDoc())
    expect(b.meta).toEqual({ v: 1, created: 1795570000000, bg: 'dots' })
    const page = b.cards['01KAR3ZC4M0000000000000001']!
    expect(page).toMatchObject({
      kind: 'page', x: 250, y: 80, w: 180, h: 390, z: 2, title: 'Mock B — flat list', state: 'live',
      viewport: { w: 390, h: 844, preset: 'phone' }, frame: '01KAR3ZC4M0000000000000009', snap: { v: 2, at: 1795570405000, theme: 'light' },
    })
    expect(page.versions).toHaveLength(2)
    expect(page.versions[1]).toEqual({
      source: { t: 'page', session: '01a20fc8-92a6-73e6-a622-3ac9e51ae923', call_id: 'toolu_01BBB' }, at: 1795570400000,
      by: 'session:01a20fc8-92a6-73e6-a622-3ac9e51ae923', note: "A's header, filter chips",
    })
    expect(b.cards['01KAR3ZC4M0000000000000002']!.versions[0]!.source).toEqual({ t: 'sticky', text: '- Triage with one thumb\n- Never lose a decision' })
    expect(b.links['01KAR3ZC4M0000000000000005']).toEqual({
      id: '01KAR3ZC4M0000000000000005', from: '01KAR3ZC4M0000000000000002', to: '01KAR3ZC4M0000000000000001', label: 'drives', style: 'solid', arrow: 'end', by: 'owner',
    })
    expect(b.frames['01KAR3ZC4M0000000000000009']).toMatchObject({ title: 'Variants', x: 36, y: 48, w: 610, h: 424, colour: 'neutral', preset: 'phone' })
    const s = b.ink['01KAR3ZC4M0000000000000007']!
    expect([...s.pts]).toEqual([...hexBytes(EXAMPLE_PTS_HEX)])
    expect(s).toMatchObject({ n: 3, o: [100, 50], bb: [98.1, 47.6, 4.8, 4.05], tool: 'pen', colour: 'red', width: 2.8, p: 1, card: '01KAR3ZC4M0000000000000001', ver: 1, cs: [180, 390] })
    expect(b.marks['01KAR3ZC4M0000000000000008']).toMatchObject({ state: 'answered', region: { kind: 'page', label: 'tab bar, item 3', role: 'tab' }, blob: true })
    expect(b.aliases['01KAR3ZC4M0000000000000008']).toBe('m4')
    expect(b.markOf['01KAR3ZC4M0000000000000007']).toBe('01KAR3ZC4M0000000000000008')
    expect(lifeOf(b, '01KAR3ZC4M0000000000000007')).toBe('answered')
  })

  it('writes/compares canvas/fixtures/example-v1.update.b64 (the Rust side reads the same bytes)', () => {
    const b64 = Buffer.from(Y.encodeStateAsUpdate(exampleDoc())).toString('base64')
    if (!existsSync(FIXTURE)) writeFileSync(FIXTURE, `${b64}\n`)
    expect(readFileSync(FIXTURE, 'utf8').trim()).toBe(b64)
    const doc = new Y.Doc()
    Y.applyUpdate(doc, Buffer.from(b64, 'base64'))
    expect(readBoard(doc)).toEqual(readBoard(exampleDoc()))
  })

  it('tolerant: unknown keys/enums are ignored on read, unparseable entries skipped and never deleted', () => {
    const doc = exampleDoc()
    doc.transact(() => {
      const page = doc.getMap<Y.Map<unknown>>('cards').get('01KAR3ZC4M0000000000000001')!
      page.set('future_field', { a: 1 })
      page.set('state', 'levitating')
      const bad = new Y.Map<unknown>()
      bad.set('kind', 'hologram')
      doc.getMap<unknown>('cards').set('01KAR3ZC4M00000000000000ZZ', bad)
      doc.getMap<unknown>('meta').set('v', BigInt(1) as unknown as number)
    })
    const b = readBoard(doc)
    expect(b.cards['01KAR3ZC4M0000000000000001']!.state).toBe('live')
    expect(b.cards['01KAR3ZC4M00000000000000ZZ']).toBeUndefined()
    expect(doc.getMap('cards').has('01KAR3ZC4M00000000000000ZZ')).toBe(true)
    expect(b.meta.v).toBe(1)
  })
})
