import { afterEach, expect, test, vi } from 'vitest'
import { LOCAL } from '@/link/engines'
import type { Api } from '@/rpc/bindings/client'
import { type ConnState, RpcError } from '@/rpc/client'
import { forgetPrivate, readText, readTextOnline, resolveDocument, resolveFile } from './files'
import { usePrivate } from './private'

// A local desktop shell: the one configuration where `resolveFile` hands out `asset://` URLs.
const convertFileSrc = vi.hoisted(() => vi.fn((p: string) => `asset://localhost${p}`))
vi.mock('@tauri-apps/api/core', () => ({ convertFileSrc }))
vi.mock('./platform', async (actual) => ({ ...(await actual<typeof import('./platform')>()), isDesktop: () => true }))

afterEach(() => vi.restoreAllMocks())

test('documents_never_use_asset_urls', async () => {
  const bytes = new TextEncoder().encode('%PDF-1.7 not really')
  const api = { get_attachment: vi.fn(async () => btoa(String.fromCharCode(...bytes))) } as unknown as Api
  const path = '/Users/x/Library/Application Support/theuth/artifacts/abc.pdf'
  // Control: an `<img>` on the local daemon does take the asset protocol.
  expect(resolveFile(api, path, LOCAL)).toBe(`asset://localhost${path}`)
  expect(convertFileSrc).toHaveBeenCalledTimes(1)

  const create = vi.spyOn(URL, 'createObjectURL')
  const url = await resolveDocument(api, path)
  expect(url.startsWith('blob:')).toBe(true)
  expect(convertFileSrc).toHaveBeenCalledTimes(1)
  expect(api.get_attachment).toHaveBeenCalledWith(path)
  const blob = create.mock.calls[0]?.[0] as Blob
  expect(blob.type).toBe('application/pdf')
  expect(new Uint8Array(await blob.arrayBuffer())).toEqual(bytes)
})

/** A client whose state the test drives; `onState` calls back at once like the real one and settles `subscribed`. */
function fakeClient(initial: ConnState) {
  const subs = new Set<(s: ConnState) => void>()
  const { promise: subscribed, resolve } = Promise.withResolvers<void>()
  return {
    subs,
    subscribed,
    onState(cb: (s: ConnState) => void) {
      subs.add(cb)
      cb(initial)
      resolve()
      return () => subs.delete(cb)
    },
    set(s: ConnState) {
      for (const cb of subs) cb(s)
    },
  }
}

test('readTextOnline: a not-connected read is not cached as a failure; it retries once the client reports open', async () => {
  const b64 = btoa('# morning')
  const get_attachment = vi
    .fn<(p: string) => Promise<string>>()
    .mockRejectedValueOnce(new RpcError(-32300, 'not connected'))
    .mockResolvedValueOnce(b64)
  const client = fakeClient('reconnecting')
  let settled = false
  const p = readTextOnline({ get_attachment } as unknown as Api, client, '/a/retry.md').finally(() => (settled = true))
  await client.subscribed
  expect(get_attachment).toHaveBeenCalledTimes(1)
  expect(settled).toBe(false)
  expect(client.subs.size).toBe(1)
  client.set('open')
  await expect(p).resolves.toBe('# morning')
  expect(get_attachment).toHaveBeenCalledTimes(2)
  // The one-shot listener is gone, and the second read is the cached one.
  expect(client.subs.size).toBe(0)
  await expect(readTextOnline({ get_attachment } as unknown as Api, client, '/a/retry.md')).resolves.toBe('# morning')
  expect(get_attachment).toHaveBeenCalledTimes(2)
})

test('readTextOnline: an already-open client retries at once; any other failure rejects without waiting', async () => {
  const get_attachment = vi
    .fn<(p: string) => Promise<string>>()
    .mockRejectedValueOnce(new RpcError(-32300, 'connection closed'))
    .mockResolvedValueOnce(btoa('x'))
  const open = fakeClient('open')
  await expect(readTextOnline({ get_attachment } as unknown as Api, open, '/a/open.md')).resolves.toBe('x')
  expect(open.subs.size).toBe(0)

  const missing = { get_attachment: vi.fn(async () => { throw new RpcError(-32000, 'no such file') }) } as unknown as Api
  const idle = fakeClient('reconnecting')
  await expect(readTextOnline(missing, idle, '/a/missing.md')).rejects.toThrow('no such file')
  expect(idle.subs.size).toBe(0)
})

test('private paths: get_attachment names the owning session and never asset://; the cache goes with the tree', async () => {
  const ROOT = 'root-session'
  const CHILD = 'child-session'
  const scratch = `/var/folders/x/T/theuth-private-${'0a'.repeat(16)}`
  // The engine's rule (51 §4.2): a scratch path is served only when its session is named; a durable one session-less.
  const get_attachment = vi.fn(async (path: string, session?: string) => {
    if (path.startsWith(scratch) !== (session !== undefined)) throw new RpcError(-32000, `${path}: not an attachment`)
    return btoa(`bytes of ${path}`)
  })
  const api = { get_attachment } as unknown as Api
  usePrivate.setState({ current: { engine: LOCAL, root: ROOT, session: ROOT, secret: '' } })

  // A renderer without a session (held strip, queued chips): the owned tree's root, over RPC even on the local daemon.
  const page = `${scratch}/artifacts/abc.html`
  const url = resolveFile(api, page, LOCAL)
  expect(typeof url).not.toBe('string')
  expect(await url).toMatch(/^blob:/)
  expect(get_attachment).toHaveBeenLastCalledWith(page, ROOT)
  expect(convertFileSrc).not.toHaveBeenCalledWith(page)

  // An explicit session (the artifact's own, a child of the tree) wins.
  const md = `${scratch}/artifacts/def.md`
  await expect(readText(api, md, CHILD)).resolves.toBe(`bytes of ${md}`)
  expect(get_attachment).toHaveBeenLastCalledWith(md, CHILD)
  await readText(api, md, CHILD)
  expect(get_attachment).toHaveBeenCalledTimes(2)

  // A durable artifact keeps the one-argument call.
  const durable = '/data/artifacts/ghi.md'
  await expect(readText(api, durable)).resolves.toBe(`bytes of ${durable}`)
  expect(get_attachment.mock.lastCall).toEqual([durable])

  // Leaving the tree: its cached bodies are gone (the next read refetches), the durable one stays cached.
  usePrivate.getState().clear()
  forgetPrivate()
  await expect(readText(api, md, CHILD)).resolves.toBe(`bytes of ${md}`)
  expect(get_attachment).toHaveBeenCalledTimes(4)
  await readText(api, durable)
  expect(get_attachment).toHaveBeenCalledTimes(4)
  // No owned tree any more: a session-less scratch read is session-less, and the engine refuses it.
  await expect(readText(api, `${scratch}/artifacts/jkl.md`)).rejects.toThrow('not an attachment')
})
