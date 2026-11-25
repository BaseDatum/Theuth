import { describe, expect, it, vi } from 'vitest'
import type { SendPlan } from '../canvas/api'
import { type SendDeps, type SendInput, sendBody, sendWhiteboard, withoutImage } from './send'

const DOC = '01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b'
const rect = (x: number, y: number, w: number, h: number) => ({ x, y, width: w, height: h, left: x, top: y, right: x + w, bottom: y + h }) as DOMRect

const PLAN: SendPlan = {
  marks: [
    { id: 'M1', strokes: ['S1'], card: 'C1', ver: 1, bb: [0, 0, 10, 10] },
    { id: 'M2', strokes: ['S2'], bb: [500, 500, 20, 20] },
  ],
  images: [
    { card: 'C1', marks: ['M1'], rect: rect(10, 10, 200, 300), world: [0, 0, 180, 390] },
    { card: null, marks: ['M2'], rect: null, world: [476, 476, 160, 160] },
  ],
  offScreen: 0,
}

function rig(over: Partial<SendDeps> = {}) {
  const calls: string[] = []
  const log =
    (name: string, ret?: unknown) =>
    (...args: unknown[]) => {
      calls.push(`${name}${name === 'blobPut' ? `:${args[1]}` : ''}`)
      return Promise.resolve(ret)
    }
  const png = new Blob([new Uint8Array([137, 80, 78, 71])], { type: 'image/png' })
  const commitSend = vi.fn(() => void calls.push('commitSend'))
  const deps: SendDeps = {
    handle: {
      planSend: vi.fn(() => (calls.push('planSend'), PLAN)),
      composeImage: vi.fn(async () => (calls.push('compose'), png)),
      commitSend,
      visible: () => ({ frames: ['F1'], cards: ['C1', 'C2'] }),
    },
    capture: vi.fn(async () => (calls.push('capture'), png)),
    recognize: vi.fn(async () => (calls.push('recognize'), { text: 'no third\ntab', confidence: 0.9 })),
    wb: { blobPut: log('blobPut'), bind: log('bind'), sendContext: vi.fn(log('sendContext')) },
    putAttachment: vi.fn(log('putAttachment', '/att/x.png')) as SendDeps['putAttachment'],
    sendInput: vi.fn(log('sendInput', 'NEW-SESSION')) as SendDeps['sendInput'],
    steer: vi.fn(log('steer')),
    ...over,
  }
  return { deps, calls, commitSend }
}

const input = (over: Partial<SendInput> = {}): SendInput => ({
  doc: DOC,
  title: 'Attention inbox',
  cards: 5,
  text: 'Drop that tab.',
  session: 'S',
  running: false,
  bound: false,
  vision: true,
  selection: ['C1'],
  clientId: 'cid',
  ...over,
})

describe('sendWhiteboard', () => {
  it('runs the contract order: plan, images, handwriting, mark blobs, attachments, commit, bind, context, send', async () => {
    const { deps, calls } = rig()
    const r = await sendWhiteboard(deps, input())
    expect(calls).toEqual(['planSend', 'capture', 'compose', 'recognize', 'blobPut:mark/M1', 'blobPut:mark/M2', 'putAttachment', 'putAttachment', 'bind', 'sendContext', 'sendInput', 'commitSend'])
    expect(r.session).toBe('NEW-SESSION')
    expect(deps.sendInput).toHaveBeenCalledWith('S', sendBody(DOC, 'Attention inbox', 5, 'Drop that tab.'), ['/att/x.png', '/att/x.png'], 'cid')
    expect(deps.wb.sendContext).toHaveBeenCalledWith('S', DOC, {
      in_view: ['F1', 'C1', 'C2'],
      selected: ['C1'],
      images: [
        { card: 'C1', marks: ['M1'] },
        { card: null, marks: ['M2'] },
      ],
      omitted: 0,
      omitted_why: null,
    })
  })

  it('handwriting only for an image of one free mark, sanitised', async () => {
    const { deps, commitSend } = rig()
    await sendWhiteboard(deps, input())
    expect(deps.recognize).toHaveBeenCalledTimes(1)
    expect(commitSend).toHaveBeenCalledWith(PLAN, { to: 'NEW-SESSION', hw: { M2: 'no third tab' }, blobs: ['M1', 'M2'] })
  })

  it('no session (R1): context keyed by the whiteboard alone, send_input(null) creates the session, nothing is bound', async () => {
    const { deps, calls } = rig()
    const r = await sendWhiteboard(deps, input({ session: null }))
    expect(calls).not.toContain('bind')
    expect(deps.wb.sendContext).toHaveBeenCalledWith(null, DOC, expect.anything())
    expect(deps.sendInput).toHaveBeenCalledWith(null, expect.stringContaining(`[attached whiteboard ${DOC} "Attention inbox" · 5 cards]`), expect.any(Array), 'cid')
    expect(r.session).toBe('NEW-SESSION')
  })

  it('a running session is steered, and an already bound one is not bound again', async () => {
    const { deps, calls } = rig()
    const r = await sendWhiteboard(deps, input({ running: true, bound: true }))
    expect(calls).not.toContain('bind')
    expect(calls.slice(-2)).toEqual(['steer', 'commitSend'])
    expect(r.session).toBe('S')
  })

  it('no vision: no files, the images are counted as omitted, mark blobs are still stored', async () => {
    const { deps, calls } = rig()
    await sendWhiteboard(deps, input({ vision: false }))
    expect(calls).not.toContain('putAttachment')
    expect(calls).toContain('blobPut:mark/M1')
    expect(deps.sendInput).toHaveBeenCalledWith('S', expect.any(String), [], 'cid')
    expect(deps.wb.sendContext).toHaveBeenCalledWith('S', DOC, expect.objectContaining({ images: [], omitted: 2, omitted_why: 'no vision' }))
  })

  it('a failed send leaves the marks fresh: commitSend never runs (review M2)', async () => {
    for (const fail of ['bind', 'sendContext', 'sendInput'] as const) {
      const { deps, commitSend } = rig()
      const boom = () => Promise.reject(new Error(`${fail} failed`))
      if (fail === 'sendInput') deps.sendInput = boom
      else deps.wb[fail] = boom
      await expect(sendWhiteboard(deps, input())).rejects.toThrow(`${fail} failed`)
      expect(commitSend).not.toHaveBeenCalled()
    }
  })

  it('a new session (R1) is the `to` of the committed marks', async () => {
    const { deps, commitSend } = rig()
    await sendWhiteboard(deps, input({ session: null }))
    expect(commitSend).toHaveBeenCalledWith(PLAN, expect.objectContaining({ to: 'NEW-SESSION' }))
  })

  it('a first send with a model pick creates the session with it, binds it and sends into it', async () => {
    const createSession = vi.fn(async () => 'PICKED')
    const { deps, calls, commitSend } = rig({ createSession })
    const sendInput = vi.fn(async () => 'PICKED')
    deps.sendInput = sendInput
    const pick = { model: 'anthropic/claude-x', effort: 'high' }
    const r = await sendWhiteboard(deps, input({ session: null, pick }))
    expect(createSession).toHaveBeenCalledWith(pick)
    expect(calls.slice(-3)).toEqual(['bind', 'sendContext', 'commitSend'])
    expect(deps.wb.sendContext).toHaveBeenCalledWith('PICKED', DOC, expect.anything())
    expect(sendInput).toHaveBeenCalledWith('PICKED', expect.any(String), expect.any(Array), 'cid')
    expect(r.session).toBe('PICKED')
    expect(commitSend).toHaveBeenCalledWith(PLAN, expect.objectContaining({ to: 'PICKED' }))
  })

  it('no pick (engine defaults) or a bound session: no create_session', async () => {
    const createSession = vi.fn(async () => 'X')
    await sendWhiteboard(rig({ createSession }).deps, input({ session: null, pick: { model: null, effort: null } }))
    await sendWhiteboard(rig({ createSession }).deps, input({ session: 'S', pick: { model: 'm', effort: null } }))
    expect(createSession).not.toHaveBeenCalled()
  })

  it('a failure before the send step creates no session (nothing left empty)', async () => {
    const createSession = vi.fn(async () => 'X')
    const { deps } = rig({ createSession })
    deps.wb.blobPut = () => Promise.reject(new Error('blob failed'))
    await expect(sendWhiteboard(deps, input({ session: null, pick: { model: 'm', effort: null } }))).rejects.toThrow('blob failed')
    expect(createSession).not.toHaveBeenCalled()
  })

  it('the marker leads the text and says "1 card" in the singular', () => {
    expect(sendBody(DOC, 'Inbox', 1, '  hi  ')).toBe(`[attached whiteboard ${DOC} "Inbox" · 1 card]\nhi`)
  })
})

describe('withoutImage', () => {
  it('removes the image and its marks', () => {
    const p = withoutImage(PLAN, 0)
    expect(p.images.map((i) => i.card)).toEqual([null])
    expect(p.marks.map((m) => m.id)).toEqual(['M2'])
  })
})
