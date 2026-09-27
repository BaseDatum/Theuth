import { expect, test } from 'vitest'
import { checkPath, parseAnchor } from './anchor'

// The same accept/reject lists as `crates/surface/src/anchor.rs` tests, so a grammar change on either side fails here.

test('grammar accepts the three code forms and block', () => {
  expect(parseAnchor('crates/surface/src/lib.rs#L71-L84')).toEqual({ kind: 'code', path: 'crates/surface/src/lib.rs', lines: [71, 84] })
  expect(parseAnchor('a/b.rs#L7')).toEqual({ kind: 'code', path: 'a/b.rs', lines: [7, 7] })
  expect(parseAnchor('a/b.rs#L7-L7')).toEqual({ kind: 'code', path: 'a/b.rs', lines: [7, 7] })
  expect(parseAnchor('README.md')).toEqual({ kind: 'code', path: 'README.md' })
  expect(parseAnchor('with space/x.rs')).toEqual({ kind: 'code', path: 'with space/x.rs' })
  expect(parseAnchor('block:b12')).toEqual({ kind: 'block', id: 'b12' })
})

test('grammar rejects escapes, schemes and bad ranges with the Rust messages', () => {
  const bad = [
    '',
    '/etc/passwd',
    '../x',
    'a/../x',
    './x',
    'a//b',
    'http://example.com/x.rs',
    'http:x',
    'file:///x',
    'C:\\x',
    'a\tb',
    'a/b.rs#L0',
    'a/b.rs#L10-L9',
    'a/b.rs#70',
    'a/b.rs#L1-',
    'a/b.rs#L1-L2-L3',
    'crates/rpc/src/api.rs:1',
    'block:',
    'block:a b',
  ]
  for (const s of bad) expect(typeof parseAnchor(s), `${JSON.stringify(s)} should not parse`).toBe('string')
  expect(parseAnchor('../x')).toBe('path must not contain "..": "../x"')
  expect(parseAnchor('http://x')).toBe('path must not carry a scheme: http:')
  expect(parseAnchor('a#L9-L3')).toBe('L3 is before L9')
  expect(parseAnchor('a/b.rs:12')).toBe('path must not contain \':\'; a line is #L<n>, not :<n>: "a/b.rs:12"')
  expect(parseAnchor('a/b.rs#70')).toBe('expected #L<a> or #L<a>-L<b>, got #70')
  expect(checkPath('a#b.rs')).not.toBeNull()
})
