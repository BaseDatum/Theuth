import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { ApprovalsOffWarning, publishArgs } from './Sharing'

const warning = (sharing: boolean, approvals: boolean | null) => renderToStaticMarkup(h(ApprovalsOffWarning, { sharing, approvals }))

test('Settings › Sharing warns when sharing is on and tools.approvals is off, only then', () => {
  expect(warning(true, false)).toContain('Approvals are off. Publishing still asks you, but other tools run without asking.')
  expect(warning(true, true)).toBe('')
  expect(warning(false, false)).toBe('')
  // Not loaded yet: no warning rather than a false one.
  expect(warning(true, null)).toBe('')
})

test('the publish form: a number is a port, anything else a folder; password access on request', () => {
  expect(publishArgs(' 5173 ', '8h', false)).toEqual({ source: { port: 5173 }, expiry: '8h', access: 'public' })
  expect(publishArgs('/srv/site', '1h', true)).toEqual({ source: { dir: '/srv/site' }, expiry: '1h', access: 'password' })
  expect(publishArgs('  ', '8h', false)).toBeNull()
})
