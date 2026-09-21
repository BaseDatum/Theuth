// 59 §14 row 4, automated proxy for the device pass (step 4 as-built): two link "devices" in this
// process — the real `HarnessProvider` over the real `LinkTransport`/Noise, fake-indexeddb standing in
// for the browser's — plus one in-process client straight to the harness, all on one note:
//
//   bun scripts/doc-link-smoke.ts --rpc ws://127.0.0.1:7801 --token $(cat $DATA/token) [--chars 50]
//
// Pairs A and B (fresh identities, `link_pair` twice), creates a note, then: type in A → seen by B
// and the local client (latency); cut B's relay socket and refuse new ones; type `--chars` in B and
// concurrently in A; restore → B replays its IndexedDB FIFO → all three docs and a fresh `doc_open`
// agree. Prints wire bytes per keystroke (device socket frames) on the way. Exit 0 = converged.

import 'fake-indexeddb/auto'
import * as Y from 'yjs'
import { HarnessProvider } from '@/lib/doc/provider'
import { generateIdentity } from '@/link/keys'
import { pair } from '@/link/pairing'
import { LinkTransport } from '@/link/transport'
import { type Api, makeApi } from '@/rpc/api'
import { RpcClient } from '@/rpc/client'
import { LocalTransport } from '@/rpc/transport'

const args = process.argv.slice(2)
const flag = (name: string) => {
  const i = args.indexOf(name)
  return i < 0 ? undefined : args[i + 1]
}
const RPC = flag('--rpc') ?? 'ws://127.0.0.1:7777'
const TOKEN = flag('--token') ?? ''
const CHARS = Number(flag('--chars') ?? 50)
const FRAGMENT = 'document-store'

/**
 * A WebSocket class per device: counts frame bytes; while `offline`, a socket that never opens and closes
 * on the next tick — what a browser does with no network (the constructor itself does not throw there).
 */
function deviceSocket() {
  const stats = { out: 0, in: 0, frames: 0, offline: false, sockets: [] as WebSocket[] }
  class Counted extends WebSocket {
    constructor(url: string | URL, protocols?: string | string[]) {
      if (stats.offline) {
        const dead = { readyState: 3, binaryType: 'arraybuffer', send() {}, close() {}, onopen: null, onclose: null, onmessage: null, onerror: null } as unknown as WebSocket
        setTimeout(() => dead.onclose?.(new CloseEvent('close', { code: 1006 })), 20)
        return dead
      }
      super(url, protocols)
      stats.sockets.push(this)
      this.addEventListener('message', (m) => {
        stats.in += (m.data as ArrayBuffer).byteLength
        stats.frames++
      })
    }
    override send(data: ArrayBuffer | ArrayBufferView | string | Blob): void {
      stats.out += typeof data === 'string' ? data.length : 'byteLength' in data ? data.byteLength : 0
      super.send(data)
    }
  }
  return { Counted: Counted as unknown as typeof WebSocket, stats }
}

const sleep = (ms: number) => {
  const { promise, resolve } = Promise.withResolvers<void>()
  setTimeout(resolve, ms)
  return promise
}

async function until(what: string, cond: () => boolean, timeoutMs = 20_000): Promise<number> {
  const t0 = performance.now()
  while (!cond()) {
    if (performance.now() - t0 > timeoutMs) throw new Error(`timeout: ${what}`)
    await sleep(5)
  }
  return Math.round(performance.now() - t0)
}

/** The first paragraph's text node in a BlockNote fragment (blockGroup → blockContainer → paragraph → text). */
function paragraphText(doc: Y.Doc): Y.XmlText {
  const walk = (el: Y.XmlFragment | Y.XmlElement): Y.XmlText | null => {
    for (const child of el.toArray()) {
      if (child instanceof Y.XmlElement && child.nodeName === 'paragraph') {
        const t = child.toArray()[0]
        if (t instanceof Y.XmlText) return t
      }
      if (child instanceof Y.XmlElement) {
        const t = walk(child)
        if (t) return t
      }
    }
    return null
  }
  const t = walk(doc.getXmlFragment(FRAGMENT))
  if (!t) throw new Error('no paragraph in the note')
  return t
}
const text = (doc: Y.Doc) => paragraphText(doc).toString()
const append = (doc: Y.Doc, s: string) => {
  const t = paragraphText(doc)
  t.insert(t.length, s)
}

async function device(name: string, local: Api) {
  const { Counted, stats } = deviceSocket()
  const identity = await generateIdentity()
  const pairing = (await local.link_pair()) as { key: string }
  const harness = await pair(pairing.key, name, { identity: () => Promise.resolve(identity) })
  const client = new RpcClient(new LinkTransport(harness, { ws: Counted, identity: () => Promise.resolve(identity) }))
  await client.connect()
  return { name, harness, client, api: makeApi(client), stats }
}

async function main(): Promise<void> {
  const localClient = RpcClient.local(RPC, TOKEN)
  await localClient.connect()
  const local = makeApi(localClient)
  const note = await local.doc_create('doc-link-smoke', '# doc-link-smoke\n\nstart\n')
  console.log(`note ${note.id}`)

  const A = await device('smoke-A', local)
  const B = await device('smoke-B', local)
  console.log(`paired A and B with ${A.harness.name} via ${A.harness.relayHost}`)

  const pA = new HarnessProvider(A.client, A.api, note.id, A.harness.id)
  const pB = new HarnessProvider(B.client, B.api, note.id, B.harness.id)
  const pL = new HarnessProvider(localClient, local, note.id) // in-process: no IndexedDB
  const live = (p: HarnessProvider) => p.status.kind === 'live'
  await until('all live', () => live(pA) && live(pB) && live(pL))
  console.log(`open: A ${JSON.stringify(text(pA.doc))} B ${JSON.stringify(text(pB.doc))} L ${JSON.stringify(text(pL.doc))}`)

  // 1. A types; B and L see it.
  const before = { out: A.stats.out, inB: B.stats.in }
  append(pA.doc, ' a1')
  const t1 = await until('a1 in B and L', () => text(pB.doc).endsWith('a1') && text(pL.doc).endsWith('a1'))
  await sleep(300)
  console.log(`A→B,L: ${t1} ms; A sent ${A.stats.out - before.out} B (device socket bytes, incl. Noise+frame overhead), B received ${B.stats.in - before.inB} B`)

  // 2. B goes offline; both keep typing.
  B.stats.offline = true
  for (const ws of B.stats.sockets) ws.close(1000)
  await until('B offline', () => pB.status.kind === 'offline')
  for (let i = 0; i < CHARS; i++) append(pB.doc, String.fromCharCode(97 + (i % 26)))
  append(pA.doc, ' A-while-B-offline')
  await until('A edit at L', () => text(pL.doc).includes('A-while-B-offline'))
  await sleep(1200) // the provider's IndexedDB save debounce
  const queued = pB.status.kind === 'offline' ? pB.status.queued : -1
  console.log(`B offline: ${queued} queued (${CHARS} typed); B text ends ${JSON.stringify(text(pB.doc).slice(-12))}`)
  if (queued !== CHARS) throw new Error(`expected ${CHARS} queued, got ${queued}`)

  // 3. Restore; B replays.
  const tRestore = performance.now()
  B.stats.offline = false
  const tLive = await until('B live again', () => live(pB), 40_000)
  const tConv = await until('converged', () => text(pA.doc) === text(pB.doc) && text(pB.doc) === text(pL.doc), 20_000)
  const opened = await local.doc_open(note.id, null)
  const fresh = new Y.Doc()
  Y.applyUpdate(fresh, Uint8Array.from(atob(opened.state.join('')), (c) => c.charCodeAt(0)))
  const same = text(fresh) === text(pA.doc)
  console.log(`B live ${tLive} ms after the network came back (RpcClient backoff included); converged +${tConv} ms; authority agrees: ${same}`)
  console.log(`final: ${JSON.stringify(text(pA.doc))}`)
  console.log(`totals — A out ${A.stats.out} in ${A.stats.in}; B out ${B.stats.out} in ${B.stats.in} (bytes on the device sockets, ${Math.round(performance.now() - tRestore)} ms since restore)`)
  if (!same || pB.status.kind !== 'live') throw new Error('not converged')
  for (const p of [pA, pB, pL]) p.destroy()
  for (const c of [A.client, B.client, localClient]) c.close()
}

main().then(
  () => process.exit(0),
  (e: unknown) => {
    console.error(e instanceof Error ? e.stack ?? e.message : e)
    process.exit(1)
  },
)
