import { afterEach, describe, expect, it, vi } from 'vitest'
import { attachNativeInk, loader } from './ink-native'

const on = { strokes: vi.fn(), event: vi.fn() }
const real = { ...loader }
afterEach(() => Object.assign(loader, real))

class FakeChannel<T> {
  onmessage: (m: T) => void = () => {}
}

describe('attachNativeInk', () => {
  it('is null outside Tauri', async () => {
    loader.tauri = () => false
    await expect(attachNativeInk(on)).resolves.toBeNull()
  })

  it('is null when the plugin module cannot load', async () => {
    loader.tauri = () => true
    loader.core = async () => null
    await expect(attachNativeInk(on)).resolves.toBeNull()
  })

  it('is null, never throws, when the plugin is missing (invoke rejects)', async () => {
    loader.tauri = () => true
    loader.core = async () => ({ invoke: () => Promise.reject(new Error('plugin ink not found')), Channel: FakeChannel }) as never
    await expect(attachNativeInk(on)).resolves.toBeNull()
  })

  it('is null when the plugin says native: false (desktop stub, older iPad)', async () => {
    loader.tauri = () => true
    loader.core = async () => ({ invoke: async () => ({ native: false, reason: 'desktop' }), Channel: FakeChannel }) as never
    await expect(attachNativeInk(on)).resolves.toBeNull()
  })

  it('is null when the loader itself throws', async () => {
    loader.tauri = () => {
      throw new Error('no window')
    }
    await expect(attachNativeInk(on)).resolves.toBeNull()
  })

  it('attached: commands go to plugin:ink, failures are swallowed, recognize failure is null', async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd === 'plugin:ink|attach') return { native: true, route: 'A' }
      throw new Error('boom')
    })
    loader.tauri = () => true
    loader.core = async () => ({ invoke, Channel: FakeChannel }) as never
    const ink = await attachNativeInk(on)
    expect(ink?.result).toEqual({ native: true, route: 'A' })
    ink?.setEnabled(false)
    ink?.ack([1, 2])
    ink?.ack([])
    await expect(ink?.recognizeText('AAAA')).resolves.toBeNull()
    await ink?.detach()
    ink?.setEnabled(true)
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(['plugin:ink|attach', 'plugin:ink|set_enabled', 'plugin:ink|ack', 'plugin:ink|recognize_text', 'plugin:ink|detach'])
  })
})
