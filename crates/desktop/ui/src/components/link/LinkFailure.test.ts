// No DOM harness here (vitest runs in node); `renderToStaticMarkup` is enough to see title, detail and Re-pair land.
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { LinkFailure } from '@/lib/link-status'
import { FailureText } from './LinkFailure'

const stale: LinkFailure = { kind: 'stale-pairing', title: 'This pairing no longer matches an engine on the relay', detail: 'Re-pair this device.', action: 'repair' }
const gone: LinkFailure = { kind: 'engine-gone', title: 'Studio Mac went away', detail: 'Waiting for it to return. Retrying in 2s.' }

test('FailureText: full and compact forms show title + detail; Re-pair only with the action', () => {
  const full = renderToStaticMarkup(createElement(FailureText, { failure: stale, onRepair: () => {} }))
  expect(full).toContain(stale.title)
  expect(full).toContain(stale.detail)
  expect(full).toContain('Re-pair')

  const compact = renderToStaticMarkup(createElement(FailureText, { failure: gone, onRepair: () => {}, compact: true }))
  expect(compact).toContain('Studio Mac went away.')
  expect(compact).toContain('Retrying in 2s.')
  expect(compact).not.toContain('Re-pair')
})
