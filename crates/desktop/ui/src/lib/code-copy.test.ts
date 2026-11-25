import { expect, test } from 'vitest'
import { copyLine, copyText } from './code-copy'

test('a cell copies without its #| header; the shebang and the code stay', () => {
  expect(copyText('bash', '#| cwd: /tmp\n#| label: Go\necho go\nls\n')).toBe('echo go\nls')
  expect(copyText('bash', '\n#!/bin/bash\n#| cwd: /tmp\n\n#| danger\necho go')).toBe('#!/bin/bash\necho go')
  expect(copyText('ts', '//| label: x\nconsole.log(1)')).toBe('console.log(1)')
  // A plain fence and a non-cell language keep every line (a `#|` in prose code is not a header there).
  expect(copyText('bash', 'echo hi\n# comment')).toBe('echo hi\n# comment')
  expect(copyText('rust', '#| not a header\nfn main() {}')).toBe('#| not a header\nfn main() {}')
})

test('$ prompts go only when every non-blank line has one', () => {
  expect(copyText('bash', '$ cd /srv\n\n$ make\n')).toBe('cd /srv\n\nmake')
  expect(copyText('bash', '$ ls\nfile.txt')).toBe('$ ls\nfile.txt')
  expect(copyText('bash', '#| cwd: /tmp\n$ ls\n$ pwd')).toBe('ls\npwd')
  expect(copyText('bash', 'echo $ HOME')).toBe('echo $ HOME')
})

test('secret placeholders pass through literally', () => {
  expect(copyText('bash', 'curl -H "Authorization: {{secret:API_KEY}}" x')).toBe('curl -H "Authorization: {{secret:API_KEY}}" x')
})

test('copyLine strips one leading $ prompt from that line alone', () => {
  expect(copyLine('$ sudo systemctl restart x')).toBe('sudo systemctl restart x')
  expect(copyLine('  $ indented')).toBe('  $ indented')
  expect(copyLine('echo $HOME')).toBe('echo $HOME')
  expect(copyLine('$x=1')).toBe('$x=1')
})
