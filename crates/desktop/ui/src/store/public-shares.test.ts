import { beforeEach, expect, test } from 'vitest'
import type { Share, ShareTraffic } from '@/components/public-share/wire'
import { activeShares, firstVisitors, newlyLive, parseTermsError, stripLabel, summarize, termsRefusal, termsToast, timeLeft, trayBadge, usePublicShares } from './public-shares'

const share = (id: string, state: Share['state'], over: Partial<Share> = {}): Share => ({
  id,
  engine: 'e1',
  edge: 'edge1',
  domain: 'tun.zip',
  host: `${id}.sky.tun.zip`,
  path: '/',
  url: `https://${id}.sky.tun.zip/`,
  source: { port: 5173 },
  access: 'public',
  interstitial: true,
  created_ms: 1,
  live_ms: state === 'pending' ? null : 1,
  expires_ms: null,
  created_by: 'settings',
  approved_by: { approval_id: '', surface: '' },
  state,
  stopped_ms: null,
  stopped_by: null,
  ...over,
})
const traffic = (id: string, visitors: number): ShareTraffic => ({ type: 'ShareTraffic', id, visitors, requests: visitors, last_ms: null })

beforeEach(() => usePublicShares.getState().reset())

test('share_list seeds the store; share.changed replaces a record in place and a stop leaves the active set', () => {
  const st = usePublicShares.getState()
  st.replace([share('a', 'live'), share('b', 'pending', { created_ms: 2 })])
  expect(usePublicShares.getState().loaded).toBe(true)
  expect(activeShares(usePublicShares.getState().shares).map((s) => s.id)).toEqual(['b', 'a'])
  st.changed(share('a', 'stopped', { stopped_by: 'user', stopped_ms: 5 }))
  st.changed(share('c', 'live'))
  expect(activeShares(usePublicShares.getState().shares).map((s) => s.id).sort()).toEqual(['b', 'c'])
  expect(Object.keys(usePublicShares.getState().shares)).toHaveLength(3)
})

test('strip label: live, pending and this engine’s visitors; nothing active means no strip', () => {
  const shares = Object.fromEntries([share('a', 'live'), share('b', 'offline'), share('c', 'pending'), share('d', 'stopped')].map((s) => [s.id, s]))
  expect(stripLabel(summarize(shares, {}))).toBe('Sharing publicly · 2 live · 1 pending')
  expect(stripLabel(summarize(shares, { a: traffic('a', 3), d: traffic('d', 9) }))).toBe('Sharing publicly · 2 live · 1 pending · 3 visitors')
  expect(stripLabel(summarize({ c: share('c', 'pending') }, {}))).toBe('Going public · 1 pending')
  expect(stripLabel(summarize({ d: share('d', 'stopped') }, {}))).toBeNull()
})

test('time left counts down to expired; no expiry is null', () => {
  const h = 3_600_000
  expect(timeLeft(null, 0)).toBeNull()
  expect(timeLeft(8 * h, 0)).toBe('8h')
  expect(timeLeft(7 * h + 12 * 60_000 + 5, 0)).toBe('7h 12m')
  expect(timeLeft(3 * 60_000 + 1, 0)).toBe('3m')
  expect(timeLeft(30_000, 0)).toBe('<1m')
  expect(timeLeft(7 * 24 * h, 0)).toBe('7d')
  expect(timeLeft(5, 10)).toBe('expired')
})

test('the engine’s terms refusal parses into edge, url and revision', () => {
  const msg =
    'share_publish: accept the terms of abc123 first (https://tun.zip/terms, revision 2026-11): Settings › Sharing or the first-run card'
  expect(parseTermsError(msg)).toEqual({ edge: 'abc123', url: 'https://tun.zip/terms', rev: '2026-11' })
  expect(parseTermsError('share_publish: no such port')).toBeNull()
})

test('TERMS-FIRST: a terms refusal is the engine’s text or a raw edge terms_required; other errors are not', () => {
  expect(termsRefusal('accept the terms of tun.zip first (https://tun.zip/terms, revision 2): x')).toEqual({ ask: { edge: 'tun.zip', url: 'https://tun.zip/terms', rev: '2' } })
  expect(termsRefusal('edge answered 403: terms_required')).toEqual({ ask: null })
  expect(termsRefusal('edge answered 409: name_taken')).toBeNull()
  expect(termsToast(null)).toBe('Accept the terms of the sharing edge first: Settings › Sharing › Domains › Terms')
})

test('notifications: newly live (not offline → live) and first visitor', () => {
  const before = { a: share('a', 'pending'), b: share('b', 'offline') }
  const after = { a: share('a', 'live'), b: share('b', 'live'), c: share('c', 'live') }
  expect(newlyLive(before, after).sort()).toEqual(['a', 'c'])
  expect(firstVisitors({ a: traffic('a', 0) }, { a: traffic('a', 1), b: traffic('b', 2) }).sort()).toEqual(['a', 'b'])
  expect(firstVisitors({ a: traffic('a', 1) }, { a: traffic('a', 4) })).toEqual([])
})

test('tray badge: LIVE leads while sharing, attention count follows; tooltip says it too', () => {
  expect(trayBadge(0, 0)).toEqual({ title: null, tooltip: 'theuth · nothing needs you' })
  expect(trayBadge(2, 0)).toEqual({ title: '●2', tooltip: 'theuth · 2 need you' })
  expect(trayBadge(1, 2)).toEqual({ title: '◉ LIVE 2 ●1', tooltip: 'theuth · sharing publicly (2) · 1 needs you' })
})
