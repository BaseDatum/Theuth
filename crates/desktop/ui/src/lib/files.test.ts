import { afterEach, expect, test, vi } from 'vitest'
import { LOCAL } from '@/link/engines'
import type { Api } from '@/rpc/api'
import { resolveDocument, resolveFile } from './files'

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
