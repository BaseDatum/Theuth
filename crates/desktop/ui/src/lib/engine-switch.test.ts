import { expect, test } from 'vitest'
import { EngineSwitch } from './engine-switch'

/** A router whose navigations land only when the test says so. */
function harness() {
  const pending: { href: string; land: () => void }[] = []
  const sw = new EngineSwitch((href) => {
    const { promise, resolve } = Promise.withResolvers<void>()
    pending.push({ href, land: resolve })
    return promise
  })
  const shown: (string | null)[] = []
  sw.subscribe(() => shown.push(sw.shown))
  return { sw, pending, shown }
}

const at = (pathname: string, search = '') => ({ pathname, href: pathname + search })

test('hydration shows at once, keeps the deep link and does not gate; a repeat (StrictMode) is a no-op', async () => {
  const { sw, pending, shown } = harness()
  expect(sw.visible('local')).toBe('local')
  await sw.activate('local', at('/link', '#abc'))
  await sw.activate('local', at('/link', '#abc'))
  expect(pending).toEqual([])
  expect(shown).toEqual(['local'])
  expect(sw.visible('local')).toBe('local')
})

test('A -> B renders nothing until B\'s navigation lands, then B; the chat left is remembered', async () => {
  const { sw, pending, shown } = harness()
  await sw.activate('A', at('/'))
  const done = sw.activate('B', at('/c/a1', '?seq=3'))
  // The old tree is gone in the same render; nothing stands in for B yet.
  expect(sw.visible('B')).toBe(null)
  expect(sw.shown).toBe(null)
  expect(pending.map((p) => p.href)).toEqual(['/'])
  expect(sw.lastChat.get('A')).toBe('/c/a1?seq=3')
  pending[0]!.land()
  await done
  expect(sw.visible('B')).toBe('B')
  expect(shown).toEqual(['A', null, 'B'])
})

test('rapid A -> B -> C: only C\'s navigation flips the gate, whichever lands first', async () => {
  const { sw, pending } = harness()
  await sw.activate('A', at('/c/a1'))
  sw.lastChat.set('B', '/c/b1')
  sw.lastChat.set('C', '/c/c1')
  const toB = sw.activate('B', at('/c/a1'))
  const toC = sw.activate('C', at('/c/a1'))
  expect(pending.map((p) => p.href)).toEqual(['/c/b1', '/c/c1'])
  // The second switch found no tree on screen: A's chat is still the one recorded.
  expect(sw.lastChat.get('A')).toBe('/c/a1')
  pending[0]!.land()
  await toB
  expect(sw.shown).toBe(null)
  expect(sw.visible('C')).toBe(null)
  pending[1]!.land()
  await toC
  expect(sw.visible('C')).toBe('C')
})

test('switching back restores the last chat; a non-chat location forgets it', async () => {
  const { sw, pending } = harness()
  await sw.activate('A', at('/'))
  const toB = sw.activate('B', at('/c/a1'))
  pending[0]!.land()
  await toB
  const back = sw.activate('A', at('/settings/mesh'))
  expect(pending[1]!.href).toBe('/c/a1')
  expect(sw.lastChat.has('B')).toBe(false)
  pending[1]!.land()
  await back
  expect(sw.visible('A')).toBe('A')
  const again = sw.activate('B', at('/c/a2'))
  expect(pending[2]!.href).toBe('/')
  pending[2]!.land()
  await again
  expect(sw.visible('B')).toBe('B')
})

test('a navigation that rejects still shows the destination', async () => {
  const sw = new EngineSwitch(() => Promise.reject(new Error('blocked')))
  await sw.activate('A', at('/'))
  await sw.activate('B', at('/c/a1'))
  expect(sw.visible('B')).toBe('B')
})
