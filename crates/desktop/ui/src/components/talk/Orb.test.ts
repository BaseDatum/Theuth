import '@/components/settings/test-dom'
import { afterEach, expect, test, vi } from 'vitest'

// The shader orb is a stand-in that renders the text `shader` and hands its failure hook to the test.
let failShader: ((error: unknown) => void) | undefined
vi.mock('@/components/orbs/orb-16', () => ({
  Orb16: ({ onError }: { onError: (error: unknown) => void }) => {
    failShader = onError
    return 'shader'
  },
}))

let unmount: (() => Promise<void>) | undefined
afterEach(async () => {
  await unmount?.()
  unmount = undefined
  failShader = undefined
  vi.unstubAllGlobals()
})

// A fresh module graph per test: the WebGPU probe is cached per session (module).
async function mount(gpu?: Partial<GPU>) {
  vi.resetModules()
  if (gpu) vi.stubGlobal('navigator', { gpu })
  const [{ act, createElement }, { createRoot }, { Orb }] = await Promise.all([import('react'), import('react-dom/client'), import('./Orb')])
  const el = document.createElement('div')
  const root = createRoot(el)
  // A zero-delay macrotask: every microtask hop of the probe (adapter → result → store) settles before it.
  const flush = () => act(() => new Promise((resolve) => setTimeout(resolve)))
  await act(async () => root.render(createElement(Orb, { state: 'idle', size: 96, onClick: () => {}, label: 'Talk' })))
  await flush()
  unmount = () => act(async () => root.unmount())
  const kind = () => (el.querySelector('.talk-orb') ? 'css' : el.textContent === 'shader' ? 'shader' : 'blank')
  return { act, flush, kind }
}

const adapter = {} as GPUAdapter

test('no navigator.gpu gives the CSS orb', async () => {
  expect((await mount()).kind()).toBe('css')
})

test('requestAdapter resolving null gives the CSS orb', async () => {
  expect((await mount({ requestAdapter: async () => null })).kind()).toBe('css')
})

test('requestAdapter throwing gives the CSS orb', async () => {
  const orb = await mount({
    requestAdapter: () => {
      throw new Error('no WebGPU')
    },
  })
  expect(orb.kind()).toBe('css')
})

test('a pending probe shows the CSS orb, then the shader once an adapter arrives', async () => {
  const pending = Promise.withResolvers<GPUAdapter | null>()
  const orb = await mount({ requestAdapter: () => pending.promise })
  expect(orb.kind()).toBe('css')
  pending.resolve(adapter)
  await orb.flush()
  expect(orb.kind()).toBe('shader')
})

test('a shader failure at runtime switches to the CSS orb', async () => {
  const orb = await mount({ requestAdapter: async () => adapter })
  expect(orb.kind()).toBe('shader')
  await orb.act(async () => failShader?.(new Error('device lost')))
  expect(orb.kind()).toBe('css')
})

test('reduced motion keeps the CSS orb even with an adapter', async () => {
  vi.stubGlobal('matchMedia', () => ({ matches: true }))
  expect((await mount({ requestAdapter: async () => adapter })).kind()).toBe('css')
})
