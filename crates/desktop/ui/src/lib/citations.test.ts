import { expect, test } from 'vitest'
import { type Cite, collectCitations, isProvenanceLine, locatorLabel, locatorMs, parseTheuthHref, routeOf } from './citations'

const cite = (href: string): Cite => parseTheuthHref(href) as Cite

test('grammar: each kind with its locator and the optional sha8', () => {
  expect(parseTheuthHref('theuth://sources/01K7S1#b12?s=0a1b2c3d')).toEqual({ href: 'theuth://sources/01K7S1#b12?s=0a1b2c3d', kind: 'sources', id: '01K7S1', locator: '#b12', sha8: '0a1b2c3d' })
  expect(parseTheuthHref('theuth://notes/n1#7f2a')).toMatchObject({ kind: 'notes', id: 'n1', locator: '#7f2a', sha8: null })
  expect(parseTheuthHref('theuth://notes/n1')).toMatchObject({ id: 'n1', locator: null })
  expect(parseTheuthHref('theuth://meetings/m1@41:12')).toMatchObject({ kind: 'meetings', id: 'm1', locator: '@41:12' })
  expect(parseTheuthHref('theuth://meetings/m1@1:05:00')).toMatchObject({ locator: '@1:05:00' })
  expect(parseTheuthHref('theuth://sources/yt1@2:03')).toMatchObject({ kind: 'sources', locator: '@2:03' })
  expect(parseTheuthHref('theuth://sessions/s1#7')).toMatchObject({ kind: 'sessions', id: 's1', locator: '#7' })
  expect(parseTheuthHref('theuth://knowledge/ops/dr')).toMatchObject({ kind: 'knowledge', id: 'ops/dr', locator: null })
})

test('grammar: rejects other schemes, kinds and malformed locators or queries', () => {
  for (const bad of [
    'https://x',
    'theuth://foo/1',
    'theuth://notes/',
    'theuth://notes/n1@41:12',
    'theuth://sessions/s1#b12',
    'theuth://meetings/m1#b1',
    'theuth://sources/a/b#b1',
    'theuth://sources/s1#b1?s=XYZ',
    'theuth://sources/s1#b1?s=0a1b2c3',
    'theuth://sources/s1?x=1',
  ])
    expect(parseTheuthHref(bad), bad).toBeNull()
})

test('collectCitations keeps first-appearance order, dedupes, and skips invalid theuth links', () => {
  const md = 'A [x](theuth://sources/s1#b1) and [y](theuth://notes/n1) then [x again](theuth://sources/s1#b1) plus [bad](theuth://foo/1) and [web](https://e.com)'
  expect(collectCitations(md)).toEqual(['theuth://sources/s1#b1', 'theuth://notes/n1'])
})

test('routeOf per kind; times become t<ms> hashes, a session turn is dropped', () => {
  expect(routeOf(cite('theuth://notes/n1#b7'))).toEqual({ to: '/notes/$id', params: { id: 'n1' }, search: {}, hash: 'b7' })
  expect(routeOf(cite('theuth://sources/s1#b12'))).toEqual({ to: '/sources/$id', params: { id: 's1' }, search: {}, hash: 'b12' })
  expect(routeOf(cite('theuth://sources/s1@2:03'))).toMatchObject({ to: '/sources/$id', hash: 't123000' })
  expect(routeOf(cite('theuth://meetings/m1@41:12'))).toEqual({ to: '/meetings/$id', params: { id: 'm1' }, hash: 't2472000' })
  expect(routeOf(cite('theuth://sessions/s1#7'))).toEqual({ to: '/c/$session', params: { session: 's1' }, search: {} })
  expect(routeOf(cite('theuth://knowledge/ops/dr'))).toEqual({ to: '/knowledge/$', params: { _splat: 'ops/dr' }, search: {} })
  expect(locatorMs('@1:05:00')).toBe(3_900_000)
  // 144 §4: inside a notebook chat, notes and sources carry the way back; other kinds do not take `?nb`.
  expect(routeOf(cite('theuth://notes/n1#b7'), 'nb1')).toEqual({ to: '/notes/$id', params: { id: 'n1' }, search: { nb: 'nb1' }, hash: 'b7' })
  expect(routeOf(cite('theuth://sources/s1'), 'nb1')).toEqual({ to: '/sources/$id', params: { id: 's1' }, search: { nb: 'nb1' }, hash: undefined })
  expect(routeOf(cite('theuth://knowledge/ops/dr'), 'nb1').search).toEqual({})
})

test('locatorLabel and provenance detection', () => {
  expect(locatorLabel(cite('theuth://sources/s1#b12'))).toBe('#b12')
  expect(locatorLabel(cite('theuth://meetings/m1@41:12'))).toBe('@41:12')
  expect(locatorLabel(cite('theuth://sessions/s1#7'))).toBe('turn 7')
  expect(locatorLabel(cite('theuth://knowledge/k'))).toBe('')
  expect(isProvenanceLine('Body\n\n*Answer from [RDS drill](theuth://sessions/s1#3)*')).toBe(true)
  expect(isProvenanceLine('Body\n\n*Answer from* *[chat](theuth://sessions/s1#1)** · m · d*')).toBe(true)
  expect(isProvenanceLine('See [RDS drill](theuth://sessions/s1#3)')).toBe(false)
})
