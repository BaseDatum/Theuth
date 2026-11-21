import { beforeEach, expect, test, vi } from 'vitest'
import * as native from './web-view-native'
import { invoke } from '@tauri-apps/api/core'

const shell = vi.hoisted(() => ({ log: [] as string[], attach: null as null | { resolve: () => void; reject: (e: string) => void } }))
const env = vi.hoisted(() => ({ container: 'ios' as 'macos' | 'ios' | 'web' }))
vi.mock('@/lib/platform', async (original) => ({
  ...(await original<object>()),
  container: () => env.container,
  isDesktop: () => env.container === 'macos',
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((cmd: string, args: { visible?: boolean; width?: number }) => {
    shell.log.push(cmd === 'web_view_visible' ? `${cmd}:${args.visible}` : cmd === 'web_view_bounds' ? `${cmd}:${args.width}` : cmd)
    // The attach lands late, like a webview created on the main thread.
    if (cmd === 'web_view_attach') {
      const { promise, resolve, reject } = Promise.withResolvers<void>()
      shell.attach = { resolve, reject }
      return promise
    }
    if (cmd === 'web_view_navigate') return Promise.reject('no web view web-x')
    return Promise.resolve()
  }),
}))

const R = { x: 0, y: 52, width: 640, height: 480 }
/** The attach reached the shell (the chain runs it a microtask later). */
const attaching = () => vi.waitFor(() => expect(shell.attach).not.toBeNull())

beforeEach(() => {
  env.container = 'ios'
  shell.log = []
  shell.attach = null
})

test('the desktop and iOS shells drive native web views; a browser never reaches the shell', async () => {
  env.container = 'macos'
  expect(native.nativeWebViews()).toBe(true)
  env.container = 'ios'
  expect(native.nativeWebViews()).toBe(true)
  env.container = 'web'
  expect(native.nativeWebViews()).toBe(false)
  await native.attach('a0', 'https://example.com/', R)
  await native.navigate('a0', 'https://example.com/x')
  await native.close('a0')
  expect(shell.log).toEqual([])
})

test('calls for one label reach the shell in issue order, each after the attach before it has landed', async () => {
  const done = Promise.all([native.attach('a1', 'https://example.com/', R), native.visible('a1', false), native.close('a1')])
  await attaching()
  // Nothing overtakes the pending attach.
  expect(shell.log).toEqual(['web_view_attach'])
  shell.attach!.resolve()
  await done
  expect(shell.log).toEqual(['web_view_attach', 'web_view_visible:false', 'web_view_close'])
})

test('a frame occluded from its first render ends hidden; state set during the attach is applied once, after it', async () => {
  const attached = native.attach('a2', 'https://example.com/', R)
  void native.visible('a2', true)
  void native.visible('a2', false)
  void native.bounds('a2', { ...R, width: 500 })
  await attaching()
  shell.attach!.resolve()
  await attached
  // The attach itself went out with the newest rectangle, so no move follows it.
  expect(shell.log).toEqual(['web_view_attach', 'web_view_visible:false'])
  expect(vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'web_view_attach').at(-1)?.[1]).toMatchObject({ width: 500 })
})

test('a rejected call does not break the chain; a refused attach leaves the view to the window holding it', async () => {
  const attached = native.attach('a3', 'https://example.com/', R)
  const hide = native.visible('a3', false)
  await attaching()
  shell.attach!.reject('in-popout: popout-web-a3')
  await expect(attached).rejects.toBe('in-popout: popout-web-a3')
  await hide
  // Not attached here: no visibility or bounds sent for a view another window shows.
  expect(shell.log).toEqual(['web_view_attach'])
  await expect(native.navigate('a3', 'https://example.com/x')).rejects.toBe('no web view web-x')
  await native.close('a3')
  expect(shell.log).toEqual(['web_view_attach', 'web_view_navigate', 'web_view_close'])
})
