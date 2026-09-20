import { expect, test } from 'vitest'
import { classify, resolveLink } from './resolveLink'

test('classify', () => {
  expect(classify('')).toBe('other')
  expect(classify('#top')).toBe('anchor')
  expect(classify('HTTPS://x.y/z')).toBe('external')
  expect(classify('//cdn/x')).toBe('external')
  expect(classify('mailto:a@b')).toBe('external')
  expect(classify('/projects/theuth.md')).toBe('absolute')
  expect(classify('./theuth.md')).toBe('relative')
})

test('absolute links resolve from the bundle root', () => {
  expect(resolveLink('a/b', '/projects/theuth.md')).toBe('projects/theuth')
  expect(resolveLink('a/b', '/projects//./theuth.md#sec')).toBe('projects/theuth')
})

test('relative links resolve from the source directory', () => {
  expect(resolveLink('projects/theuth', 'tui.md')).toBe('projects/tui')
  expect(resolveLink('projects/theuth', './tui.md')).toBe('projects/tui')
  expect(resolveLink('projects/theuth/deep', '../x/y.md')).toBe('projects/x/y')
  expect(resolveLink('top', 'other.md')).toBe('other')
})

test('anchors, external, directories and empty resolve to null', () => {
  expect(resolveLink('a/b', '#sec')).toBeNull()
  expect(resolveLink('a/b', 'https://x.y/z.md')).toBeNull()
  expect(resolveLink('a/b', 'tel:123')).toBeNull()
  expect(resolveLink('a/b', '/projects/')).toBeNull()
  expect(resolveLink('a/b', 'dir/')).toBeNull()
  expect(resolveLink('a/b', '')).toBeNull()
})

test('.md is stripped from the last segment only; case is kept', () => {
  expect(resolveLink('a/b', '/x.md/y.md')).toBe('x.md/y')
  expect(resolveLink('a/b', '/Notes/README.md')).toBe('Notes/README')
  expect(resolveLink('a/b', '/x.markdown')).toBe('x.markdown')
})

test('.. at the root is a no-op, like okf; bad segments are null', () => {
  expect(resolveLink('top', '../../x.md')).toBe('x')
  expect(resolveLink('a/b', '/..')).toBeNull()
  expect(resolveLink('a/b', '/-bad/x.md')).toBeNull()
  expect(resolveLink('a/b', 'sp ace.md')).toBeNull()
})
