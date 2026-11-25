import { afterEach, describe, expect, it, vi } from 'vitest'
import { attachNativeInk, loader, resetNativeInkOwner } from './ink-native'

const on = { strokes: vi.fn(), event: vi.fn() }
const real = { ...loader }
afterEach(() => {
  Object.assign(loader, real)
  resetNativeInkOwner()
})

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
    await new Promise((r) => setTimeout(r, 0))
    await expect(ink?.recognizeText('AAAA')).resolves.toBeNull()
    await ink?.detach()
    ink?.setEnabled(true)
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(['plugin:ink|attach', 'plugin:ink|set_enabled', 'plugin:ink|ack', 'plugin:ink|recognize_text', 'plugin:ink|detach'])
  })
})

describe('overlay ownership (218 review M3)', () => {
  const rig = (block = false) => {
    const calls: string[] = []
    let release: (() => void) | null = null
    const invoke = vi.fn(async (cmd: string) => {
      calls.push(cmd.replace('plugin:ink|', ''))
      if (cmd === 'plugin:ink|attach') {
        if (block && calls.filter((c) => c === 'attach').length === 1) await new Promise<void>((r) => (release = r))
        return { native: true }
      }
      return null
    })
    loader.tauri = () => true
    loader.core = async () => ({ invoke, Channel: FakeChannel }) as never
    return { calls, release: () => release?.() }
  }

  it('a late detach from an unmounted view does not remove the next view’s overlay', async () => {
    const { calls, release } = rig(true)
    const first = attachNativeInk(on)          // view A: attach still pending
    while (!calls.includes('attach')) await new Promise((r) => setTimeout(r, 0))
    const second = attachNativeInk(on)         // view B mounts before A’s attach resolved
    release()
    const a = await first
    const b = await second
    expect(a).toBeNull()                       // superseded: A gets nothing to detach
    expect(b).not.toBeNull()
    b?.setEnabled(true)
    await new Promise((r) => setTimeout(r, 0))
    expect(calls).toEqual(['attach', 'attach', 'set_enabled'])
  })

  it('the old owner is told it lost the overlay; its commands and detach are dropped', async () => {
    rig()
    const lost = vi.fn()
    const a = await attachNativeInk({ ...on, lost })
    const { calls } = rig()
    const b = await attachNativeInk(on)
    expect(lost).toHaveBeenCalledTimes(1)
    a?.setEnabled(true)
    await a?.detach()
    b?.setTool({ tool: 'pen', colour: '#000000', width: 2 })
    await new Promise((r) => setTimeout(r, 0))
    expect(calls).toEqual(['attach', 'set_tool'])
    await b?.detach()
    expect(calls.at(-1)).toBe('detach')
  })
})
