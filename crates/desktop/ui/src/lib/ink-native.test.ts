import { afterEach, describe, expect, it, vi } from 'vitest'
import { type PencilEvent, listenPencil, loader, recognizeText } from './ink-native'

const real = { ...loader }
afterEach(() => {
  Object.assign(loader, real)
})

class FakeChannel<T> {
  onmessage: (m: T) => void = () => {}
}

/** A Tauri core whose `invoke` answers per command; every call is recorded. */
function shell(answer: (cmd: string, args: Record<string, unknown>) => unknown) {
  const calls: [string, Record<string, unknown>][] = []
  loader.tauri = () => true
  loader.core = async () =>
    ({
      Channel: FakeChannel,
      invoke: async (cmd: string, args: Record<string, unknown>) => {
        calls.push([cmd, args])
        return answer(cmd, args)
      },
    }) as never
  return calls
}

describe('recognizeText', () => {
  it('is null outside Tauri and when the core cannot load', async () => {
    loader.tauri = () => false
    await expect(recognizeText('png')).resolves.toBeNull()
    loader.tauri = () => true
    loader.core = async () => null
    await expect(recognizeText('png')).resolves.toBeNull()
  })

  it('is null when the plugin is missing or fails; never throws', async () => {
    shell(() => {
      throw new Error('plugin ink not found')
    })
    await expect(recognizeText('png')).resolves.toBeNull()
    loader.tauri = () => {
      throw new Error('no window')
    }
    await expect(recognizeText('png')).resolves.toBeNull()
  })

  it('calls plugin:ink|recognize_text alone, with no listener or attach first', async () => {
    const calls = shell(() => ({ text: 'no third tab', confidence: 0.8 }))
    await expect(recognizeText('AAAA')).resolves.toEqual({ text: 'no third tab', confidence: 0.8 })
    expect(calls).toEqual([['plugin:ink|recognize_text', { png: 'AAAA' }]])
  })
})

describe('listenPencil', () => {
  it('outside Tauri, or with the plugin missing, resolves to a no-op', async () => {
    loader.tauri = () => false
    const off = await listenPencil(() => {})
    expect(() => off()).not.toThrow()
    const calls = shell(() => {
      throw new Error('plugin ink not found')
    })
    ;(await listenPencil(() => {}))()
    expect(calls.map((c) => c[0])).toEqual(['plugin:ink|pencil_listen'])
  })

  it('ok: false (desktop, iPhone, iPadOS < 17.5) never unlistens', async () => {
    const calls = shell(() => ({ ok: false }))
    ;(await listenPencil(() => {}))()
    expect(calls.map((c) => c[0])).toEqual(['plugin:ink|pencil_listen'])
  })

  it('forwards pencil events from the channel, drops others, and unlistens once', async () => {
    let channel: FakeChannel<unknown> | null = null
    const calls = shell((cmd, args) => {
      if (cmd === 'plugin:ink|pencil_listen') channel = args.onEvent as FakeChannel<unknown>
      return { ok: true }
    })
    const on = vi.fn()
    const off = await listenPencil(on)
    const tap: PencilEvent = { kind: 'pencil', gesture: 'tap', action: 'switchEraser' }
    channel!.onmessage(tap)
    channel!.onmessage({ kind: 'error', message: 'x' })
    expect(on.mock.calls).toEqual([[tap]])
    off()
    off()
    expect(calls.map((c) => c[0])).toEqual(['plugin:ink|pencil_listen', 'plugin:ink|pencil_unlisten'])
  })

  it('a superseded listener does not unlisten the newer one (the plugin keeps one)', async () => {
    const calls = shell(() => ({ ok: true }))
    const first = await listenPencil(() => {})
    const second = await listenPencil(() => {})
    first()
    expect(calls.map((c) => c[0])).toEqual(['plugin:ink|pencil_listen', 'plugin:ink|pencil_listen'])
    second()
    expect(calls.at(-1)?.[0]).toBe('plugin:ink|pencil_unlisten')
  })
})
