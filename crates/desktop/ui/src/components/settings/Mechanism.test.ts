import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { KEY } from '@/components/autoresearch/queries'
import type { MechanismInfo } from '@/rpc/bindings/MechanismInfo'
import { Mechanism, adoptMechanism, adoptedText } from './Mechanism'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))

const info: MechanismInfo = {
  active: 2,
  versions: [
    { version: 2, sha: 'deadbeefcafe0001', at: '2026-10-09T10:00:00Z', summary: 'meta-1: +4.2 % suite score' },
    { version: 1, sha: '0123456789abcdef', at: '2026-10-01T09:00:00Z', summary: 'compiled defaults' },
  ],
  files: {},
}

const paint = (m: MechanismInfo) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  qc.setQueryData([...KEY, 'mechanism'], m)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Mechanism) }))
}

test('list: the active version, one row per version with sha short · summary, the active one badged and not adoptable', () => {
  const html = paint(info)
  expect(html).toContain('>v2<')
  expect(html).toContain('deadbee ·')
  expect(html).toContain('meta-1: +4.2 % suite score')
  expect(html).toContain('0123456 ·')
  expect(html).toContain('compiled defaults')
  expect(html).toContain('>active<')
  expect((html.match(/>Adopt</g) ?? []).length).toBe(2)
  expect((html.match(/>Show</g) ?? []).length).toBe(2)
  // Revert is offered while a version is active; with the defaults active it is disabled. The active version's Adopt is disabled too.
  const disabled = (word: string, h: string) => new RegExp(`<button[^>]* disabled=""[^>]*>${word}<`).test(h)
  expect(disabled('Revert', html)).toBe(false)
  expect(disabled('Adopt', html)).toBe(true)
  const defaults = paint({ ...info, active: null })
  expect(disabled('Revert', defaults)).toBe(true)
  expect(disabled('Adopt', defaults)).toBe(false)
  expect(defaults).toContain('>compiled defaults<')
})

test('adopt: the press sends {op: adopt, version}; Revert is version 0; the toast names what is active now', async () => {
  const calls: unknown[] = []
  const api = { 'autoresearch.mechanism': async (op: unknown) => (calls.push(op), { ...info, active: op && typeof op === 'object' && 'version' in op && op.version === 0 ? null : 1 }) }
  expect((await adoptMechanism(api, 1)).active).toBe(1)
  expect(calls).toEqual([{ op: 'adopt', version: 1 }])
  expect((await adoptMechanism(api, 0)).active).toBeNull()
  expect(calls[1]).toEqual({ op: 'adopt', version: 0 })
  expect(adoptedText({ active: 3 })).toBe('Adopted mechanism v3')
  expect(adoptedText({ active: null })).toBe('Reverted to the compiled defaults')
})
