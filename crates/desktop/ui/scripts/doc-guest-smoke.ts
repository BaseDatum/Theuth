// 59 §8.3 / §14 row 7 protocol checks, guest side, without a browser:
//   bun scripts/doc-guest-smoke.ts <share-url> [--handshakes 11] [--name NAME]
// Fresh X25519 static per run. Checks, in order: (1) a wrong proof is refused
// with `bad_proof`; (2) a correct proof joins, `doc_open` works, a viewer's
// `doc_update` is refused with `viewer_readonly` (an editor's is accepted);
// (3) N more handshakes from fresh statics inside one minute: the 11th (the
// harness's `guest_handshakes_per_minute` default is 10) gets `CLOSE 3` with no
// message 2. Exit 0 when every assertion held.

import * as Y from 'yjs'
import { joinProof, parseShareLink } from '@/link/guest'
import type { Harness } from '@/link/harnesses'
import { generateIdentity } from '@/link/keys'
import { LinkTransport } from '@/link/transport'
import { type Api, makeApi } from '@/rpc/api'
import { RpcClient, RpcError } from '@/rpc/client'

const args = process.argv.slice(2)
const flag = (name: string) => {
  const i = args.indexOf(name)
  return i < 0 ? undefined : args[i + 1]
}
const url = args.find((a) => a.startsWith('http'))
if (!url) throw new Error('usage: doc-guest-smoke.ts <share-url> [--handshakes N] [--name NAME]')
const u = new URL(url)
const target = parseShareLink(u.hash, u.host, u.protocol === 'https:')
if (!target) throw new Error('bad share link')
const harness: Harness = { id: target.harnessId, name: '', relayHost: target.relayHost, secure: target.secure, pairedAt: 1, lastSeen: 0, order: 0 }
const t0 = performance.now()
const ms = () => `${(performance.now() - t0).toFixed(0)} ms`
const log = (...a: unknown[]) => console.log(`[${ms()}]`, ...a)

type Guest = { client: RpcClient; transport: LinkTransport; closes: number[]; api: Api; pub: Uint8Array<ArrayBuffer> }

/** A fresh static, connected (or refused: `closes` holds the reason and `open` is false). */
async function guest(): Promise<Guest & { open: boolean }> {
  const id = await generateIdentity()
  const closes: number[] = []
  const transport = new LinkTransport(harness, { identity: () => Promise.resolve(id), onClose: (r) => closes.push(r) })
  const client = new RpcClient(transport)
  const api = makeApi(client)
  const refused = Promise.withResolvers<boolean>()
  const poll = setInterval(() => closes.length > 0 && refused.resolve(false), 10)
  const giveUp = setTimeout(() => refused.resolve(false), 8000)
  const opened = await Promise.race([client.connect().then(() => true, () => false), refused.promise])
  clearInterval(poll)
  clearTimeout(giveUp)
  return { client, transport, closes, api, pub: id.pub, open: opened }
}

const code = (e: unknown) => (e instanceof RpcError ? e.message : e instanceof Error ? e.message : String(e))
let failures = 0
const check = (ok: boolean, what: string) => {
  log(ok ? 'ok  ' : 'FAIL', what)
  if (!ok) failures++
}

// (1) wrong proof
{
  const g = await guest()
  check(g.open, 'guest handshake completes (unknown static → mode 2)')
  const h = g.transport.handshakeHash
  if (!h) throw new Error('no handshake hash')
  const wrong = await joinProof(new Uint8Array(32).fill(1), h, g.pub)
  const err = await g.api.doc_join(target.doc, wrong, 'Wrong').then(() => '', code)
  check(err.includes('bad_proof'), `wrong proof refused: ${err}`)
  // Off the allow-list: -32601 from link's gate, never dispatched.
  const gated = await g.api.doc_list(null, null).then(() => '', code)
  check(gated.includes('Method not found'), `doc_list gated for a guest: ${gated}`)
  // Not joined: the harness closes the conn 5 s after IK with CLOSE 3.
  await new Promise((r) => setTimeout(r, 5500))
  check(g.closes.includes(3), `unjoined conn closed with CLOSE 3 within 5 s: closes=${JSON.stringify(g.closes)}`)
  g.client.close()
}

// (2) correct proof, roles
{
  const g = await guest()
  const h = g.transport.handshakeHash
  if (!h) throw new Error('no handshake hash')
  const name = flag('--name') ?? 'Script guest'
  const joined = await g.api.doc_join(target.doc, await joinProof(target.secret, h, g.pub), name)
  log('joined', joined)
  check(typeof joined.color === 'string' && joined.color.startsWith('#'), 'joined with a palette colour')
  const opened = await g.api.doc_open(target.doc, null)
  check(opened.meta.links.length === 0, 'doc_open strips links for a member')
  check(opened.meta.members.some((m) => m.name === joined.name), 'member listed in meta.members')
  // A scratch update on an unrelated root type: valid CRDT bytes, invisible to BlockNote.
  const d = new Y.Doc()
  d.getText('scratch').insert(0, 'x')
  const update = Buffer.from(Y.encodeStateAsUpdate(d)).toString('base64')
  const upd = await g.api.doc_update(target.doc, update).then(() => 'ok', code)
  if (joined.role === 'viewer') check(upd.includes('viewer_readonly'), `viewer doc_update refused: ${upd}`)
  else check(upd === 'ok', `editor doc_update accepted: ${upd}`)
  const rename = await g.api.doc_set_name(target.doc, `${name} 2`).then(() => 'ok', code)
  if (joined.role === 'viewer') check(rename.includes('viewer_readonly'), `viewer doc_set_name refused: ${rename}`)
  else check(rename === 'ok', `editor doc_set_name accepted: ${rename}`)
  await new Promise((r) => setTimeout(r, 5500))
  check(!g.closes.includes(3), `joined conn stays open past 5 s: closes=${JSON.stringify(g.closes)}`)
  g.client.close()
}

// (3) rate limit: the two handshakes above count; N more fresh statics.
{
  const n = Number(flag('--handshakes') ?? 11)
  const results: string[] = []
  for (let i = 0; i < n; i++) {
    const g = await guest()
    results.push(g.open ? 'open' : `close ${g.closes[0]}`)
    g.client.close()
  }
  log('handshakes:', results.join(', '))
  const refused = results.filter((r) => r === 'close 3').length
  check(refused >= 1 && results.slice(0, 8).every((r) => r === 'open'), `2 + ${n} guest handshakes in a minute: ${refused} refused before message 2 (budget 10/min)`)
}

log(failures === 0 ? 'ALL OK' : `${failures} FAILED`)
process.exit(failures === 0 ? 0 : 1)
