import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { ActivityChip, waitingLabel } from './identity'

const chip = (waiting?: string[]) => renderToStaticMarkup(createElement(ActivityChip, { status: 'working', waiting }))

test('178: the chip names the child a desk waits on, then how many more', () => {
  expect(waitingLabel(['scout'])).toBe('waiting on scout')
  expect(waitingLabel(['scout', 'probe', 'docs'])).toBe('waiting on scout +2')
  expect(waitingLabel([])).toBeNull()
  expect(waitingLabel(undefined)).toBeNull()
  expect(chip(['scout', 'probe', 'docs'])).toContain('>waiting on scout +2<')
  expect(chip(['scout'])).toContain('>waiting on scout<')
})

test('178: no waiting children: the bare status', () => {
  expect(chip([])).toContain('>working<')
  expect(chip()).toContain('>working<')
  expect(chip()).not.toContain('waiting on')
})
