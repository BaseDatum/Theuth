// 218 §8.8 "Send": everything the composer does between the Send button and the engine, in the contract's order —
// plan → images within the budget (native capture, else the canvas's composition) → handwriting → mark blobs →
// attachments → commitSend (marks: sent; earlier sent → answered) → bind → send context → send_input / steer.
// Dependencies are injected so the order and the no-session / no-vision paths are testable without a DOM.

import { compose, whiteboardRef } from '@/lib/attach'
import type { Id } from '../canvas/model'
import type { PlannedImage, SendPlan, WhiteboardCanvasHandle } from '../canvas/api'
import type { DocId, SessionId, WbSendContext, WbSentImage } from '../wire'
import { type BudgetImage, fitBudget } from './budget'

/** A handwriting answer below this confidence is not sent (it would be noise the agent has to second-guess). */
export const HW_CONFIDENCE = 0.5
const IN_VIEW_CAP = 12

export type SendDeps = {
  handle: Pick<WhiteboardCanvasHandle, 'planSend' | 'composeImage' | 'commitSend' | 'visible'>
  /** `CanvasHost.capture`: native pixels of a window rect, null where there are none. */
  capture(rect: { x: number; y: number; width: number; height: number }, maxSide: number): Promise<Blob | null>
  /** `NativeInk.recognizeText`, when the native plugin is attached. */
  recognize?: (png: string) => Promise<{ text: string; confidence: number } | null>
  wb: {
    blobPut(doc: DocId, key: string, mime: string, data: string): Promise<unknown>
    bind(session: SessionId, doc: DocId): Promise<unknown>
    sendContext(session: SessionId | null, doc: DocId, context: WbSendContext): Promise<unknown>
  }
  putAttachment(blob: Blob, ext: string, session?: string): Promise<string>
  sendInput(session: SessionId | null, body: string, files: string[], clientId: string): Promise<string>
  steer(session: SessionId, body: string, clientId: string, files: string[]): Promise<unknown>
  /** Device pixels per CSS px (natural size of a captured rect). */
  dpr?: number
}

export type SendInput = {
  doc: DocId
  title: string
  /** Live + dimmed cards: the marker's `N cards`. */
  cards: number
  text: string
  session: SessionId | null
  running: boolean
  /** The session is already bound to this whiteboard. */
  bound: boolean
  /** The session's model (or a vision fallback) can see images. */
  vision: boolean
  selection: Id[]
  clientId: string
  /** The plan the tray showed (with removals); planned fresh when absent. */
  plan?: SendPlan
  /** Card id → frame id, for merging per frame. */
  frameOf?: (card: Id) => Id | undefined
}

export type SendResult = { session: SessionId; plan: SendPlan; images: number; omitted: number }

const EXT: Record<string, string> = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/webp': 'webp' }

/** Blob → base64 (no prefix), chunked so a large image never overflows the argument list. */
export async function blobBase64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer())
  let bin = ''
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(bin)
}

/** The tray's "×": the image goes, and so do its marks (they stay fresh, for a later send) and their zoom images. */
export function withoutImage(plan: SendPlan, index: number): SendPlan {
  const gone = plan.images[index]
  if (!gone) return plan
  const marks = new Set(gone.marks)
  return {
    ...plan,
    marks: plan.marks.filter((m) => !marks.has(m.id)),
    images: plan.images.filter((i, k) => k !== index && !(i.zoomOf && marks.has(i.zoomOf)) && !(i.card === null && i.marks.every((m) => marks.has(m)) && i.marks.length > 0)),
  }
}

/** The body: the whiteboard marker leads (so the id survives compaction and binds a new session), then the words. */
export function sendBody(doc: DocId, title: string, cards: number, text: string): string {
  return compose([whiteboardRef(doc, title, cards)], text.trim())
}

export async function sendWhiteboard(d: SendDeps, i: SendInput): Promise<SendResult> {
  const plan = i.plan ?? d.handle.planSend()
  const dpr = d.dpr ?? 1
  // 1. Images within the budget.
  const asBudget: BudgetImage[] = plan.images.map((p) => ({
    card: p.card,
    marks: p.marks,
    world: p.world,
    rect: p.rect,
    zoomOf: p.zoomOf,
    frame: p.card ? i.frameOf?.(p.card) : undefined,
    px: p.rect ? [p.rect.width * dpr, p.rect.height * dpr] : [p.world[2], p.world[3]],
  }))
  const budget = fitBudget(asBudget)
  const blobs: { image: PlannedImage; blob: Blob }[] = []
  for (const b of budget.images) {
    const maxSide = Math.max(b.w, b.h)
    const image: PlannedImage = { card: b.card, marks: b.marks, rect: b.rect, world: b.world, zoomOf: b.zoomOf }
    const native = b.rect ? await d.capture({ x: b.rect.x, y: b.rect.y, width: b.rect.width, height: b.rect.height }, maxSide) : null
    blobs.push({ image, blob: native ?? (await d.handle.composeImage(image, maxSide)) })
  }
  // 2. Handwriting (R6): only an image of one free mark is ink alone — on a card the text would be the page's.
  const hw: Record<Id, string> = {}
  if (d.recognize) {
    for (const { image, blob } of blobs) {
      const [only] = image.marks
      if (image.card !== null || image.zoomOf || image.marks.length !== 1 || !only) continue
      const r = await d.recognize(await blobBase64(blob)).catch(() => null)
      const text = r && r.confidence >= HW_CONFIDENCE ? r.text.replace(/[\r\n[\]<>]+/g, ' ').trim().slice(0, 200) : ''
      if (text) hw[only] = text
    }
  }
  // 3. Mark blobs: the pixel record `wb_crop(mark=…)` reads; a zoom image (later in the plan) overwrites its card's.
  const stored = new Set<Id>()
  for (const { image, blob } of blobs) {
    const data = await blobBase64(blob)
    for (const m of image.marks) {
      await d.wb.blobPut(i.doc, `mark/${m}`, blob.type || 'image/png', data)
      stored.add(m)
    }
  }
  // 4. Attachments — none when the model cannot see them.
  const files: string[] = []
  const sent: WbSentImage[] = []
  if (i.vision) {
    for (const { image, blob } of blobs) {
      files.push(await d.putAttachment(blob, EXT[blob.type] ?? 'png', i.session ?? undefined))
      sent.push({ card: image.card, marks: image.marks })
    }
  }
  // 5. The marks become `sent` (one wb-local transaction, B3).
  d.handle.commitSend(plan, { to: i.session, hw, blobs: [...stored] })
  // 6. Bind, then the context the injection consumes.
  if (i.session && !i.bound) await d.wb.bind(i.session, i.doc)
  const seen = d.handle.visible()
  const omitted = i.vision ? budget.omitted : blobs.length + budget.omitted
  const context: WbSendContext = {
    in_view: [...seen.frames, ...seen.cards].slice(0, IN_VIEW_CAP),
    selected: i.selection,
    images: sent,
    omitted,
    omitted_why: omitted === 0 ? null : i.vision ? 'budget' : 'no vision',
  }
  await d.wb.sendContext(i.session, i.doc, context)
  // 7. The message.
  const body = sendBody(i.doc, i.title, i.cards, i.text)
  let session = i.session
  if (session && i.running) await d.steer(session, body, i.clientId, files)
  else session = await d.sendInput(session, body, files, i.clientId)
  return { session: session ?? '', plan, images: sent.length, omitted }
}
