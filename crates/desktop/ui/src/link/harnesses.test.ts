// Every desktop shell has a daemon beside it (31 §7.7): the local row is listed, it is the default
// `active`, and `useNeedsPairing` never sends it to `/link`. A plain browser has none.
import { afterEach, expect, test, vi } from 'vitest'

vi.mock('./idb', () => ({ idbGet: () => Promise.resolve(undefined), idbPut: () => Promise.resolve() }))

const UA = {
  macos: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)',
  linux: 'Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15',
  windows: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0',
}

/** A fresh module per shell: `LOCAL_NAME` and the hydration promise are computed once at import. */
async function load(ua: string, tauri = true) {
  vi.stubGlobal('window', tauri ? { __TAURI_INTERNALS__: {} } : {})
  vi.stubGlobal('navigator', { userAgent: ua, maxTouchPoints: 0 })
  vi.stubEnv('VITE_THEUTH_URL', undefined)
  vi.stubEnv('VITE_THEUTH_TOKEN', undefined)
  vi.resetModules()
  return import('./harnesses')
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.unstubAllEnvs()
})

test.each([
  ['macos', 'This Mac'],
  ['linux', 'This computer'],
  ['windows', 'This PC'],
] as const)('%s shell: local daemon, no pairing demanded, label %s', async (os, label) => {
  const m = await load(UA[os])
  expect(m.hasLocal()).toBe(true)
  expect(m.LOCAL_NAME).toBe(label)
  await m.hydrateHarnesses()
  const s = m.useHarnesses.getState()
  expect(s).toMatchObject({ hydrated: true, harnesses: [], active: m.LOCAL })
  // `useNeedsPairing` = hydrated && no harnesses && !hasLocal(): the last term keeps the pair screen away.
  expect(s.hydrated && s.harnesses.length === 0 && !m.hasLocal()).toBe(false)
})

test('browser without a dev daemon: no local row, pairing needed once hydrated empty', async () => {
  const m = await load(UA.linux, false)
  expect(m.hasLocal()).toBe(false)
  await m.hydrateHarnesses()
  const s = m.useHarnesses.getState()
  expect(s.hydrated && s.harnesses.length === 0 && !m.hasLocal()).toBe(true)
})
