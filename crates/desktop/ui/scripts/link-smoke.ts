// 31 §12 step-4 check, device side, without a browser:
//   bun scripts/link-smoke.ts <pairing-key-or-url> [--name NAME]     pair, then run a session
//   bun scripts/link-smoke.ts --session <harnessId>                   reuse ./.link-smoke-identity.json
// Pairs (or reuses the saved identity + harness), calls `list_sessions`, subscribes,
// prints the first 5 events (or whatever arrives within --wait seconds) and exits 0.
// The identity file holds a raw private key — for this script only; the app keeps
// its key non-extractable inside IndexedDB.

import { readFileSync, writeFileSync } from 'node:fs'
import type { Harness } from '@/link/harnesses'
import { type Identity, generateIdentity, importPrivate } from '@/link/keys'
import { pair } from '@/link/pairing'
import { LinkTransport } from '@/link/transport'
import { RpcClient } from '@/rpc/client'

const FILE = './.link-smoke-identity.json'
type Saved = { priv: string; pub: string; harnesses: Record<string, Harness> }

const hex = (b: Uint8Array<ArrayBuffer>) => Buffer.from(b).toString('hex')
const args = process.argv.slice(2)
const flag = (name: string) => {
  const i = args.indexOf(name)
  return i < 0 ? undefined : args[i + 1]
}
const positional = args.filter((a, i) => !a.startsWith('--') && !args[i - 1]?.startsWith('--'))

function load(): Saved | null {
  try {
    return JSON.parse(readFileSync(FILE, 'utf8')) as Saved
  } catch {
    return null
  }
}

async function identityOf(saved: Saved): Promise<Identity> {
  return { priv: await importPrivate(Uint8Array.from(Buffer.from(saved.priv, 'hex'))), pub: Uint8Array.from(Buffer.from(saved.pub, 'hex')) }
}

async function freshSaved(): Promise<Saved> {
  const id = await generateIdentity(true)
  const pkcs8 = new Uint8Array(await crypto.subtle.exportKey('pkcs8', id.priv))
  return { priv: hex(pkcs8.subarray(-32)), pub: hex(id.pub), harnesses: {} }
}

async function main(): Promise<void> {
  const wait = Number(flag('--wait') ?? 15) * 1000
  let saved = load()
  let harness: Harness
  const sessionId = flag('--session')
  if (sessionId) {
    const h = saved?.harnesses[sessionId]
    if (!saved || !h) throw new Error(`no saved pairing for ${sessionId} in ${FILE}`)
    harness = h
  } else {
    const key = positional[0]
    if (!key) throw new Error('usage: link-smoke.ts <pairing-key-or-url> [--name NAME] | --session <harnessId>')
    saved ??= await freshSaved()
    const id = await identityOf(saved)
    harness = await pair(key, flag('--name') ?? 'link-smoke', { identity: () => Promise.resolve(id) })
    saved.harnesses[harness.id] = harness
    writeFileSync(FILE, JSON.stringify(saved, null, 2), { mode: 0o600 })
    console.log(`paired with ${harness.name} (${harness.id}) via ${harness.relayHost}`)
  }
  const id = await identityOf(saved)
  const transport = new LinkTransport(harness, { identity: () => Promise.resolve(id) })
  const client = new RpcClient(transport)
  client.onState((s) => console.log(`state: ${s}`))
  await client.connect()
  console.log(`hello: ${JSON.stringify(transport.hello)}`)
  console.log(`list_sessions: ${JSON.stringify(await client.call('list_sessions', []))}`)
  let n = 0
  const done = Promise.withResolvers<void>()
  const off = await client.subscribe(null, null, (e) => {
    console.log(`event ${++n}: ${JSON.stringify(e)}`)
    if (n >= 5) done.resolve()
  })
  const timer = setTimeout(() => {
    console.log(`no more events within ${wait / 1000}s`)
    done.resolve()
  }, wait)
  await done.promise
  clearTimeout(timer)
  await off()
  client.close()
}

main().then(
  () => process.exit(0),
  (e: unknown) => {
    console.error(e instanceof Error ? e.message : e)
    process.exit(1)
  },
)
