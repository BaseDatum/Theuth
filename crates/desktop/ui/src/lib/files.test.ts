import { afterEach, expect, test, vi } from 'vitest'
import { LOCAL } from '@/link/engines'
import type { Api } from '@/rpc/api'
import { type ConnState, RpcError } from '@/rpc/client'
import { readTextOnline, resolveDocument, resolveFile } from './files'

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
