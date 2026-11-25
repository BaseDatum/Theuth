import { afterEach, expect, test, vi } from 'vitest'
import { copyLine, copyText, copyToClipboard } from './code-copy'

const notify = vi.hoisted(() => ({ message: vi.fn(), error: vi.fn() }))
vi.mock('@/lib/notify', () => ({ notify }))

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

test('a cell copies without its #| header; the shebang and the code stay', () => {
  expect(copyText('bash', '#| cwd: /tmp\n#| label: Go\necho go\nls\n')).toBe('echo go\nls')
  expect(copyText('bash', '\n#!/bin/bash\n#| cwd: /tmp\n\n#| danger\necho go')).toBe('#!/bin/bash\necho go')
  expect(copyText('ts', '//| label: x\nconsole.log(1)')).toBe('console.log(1)')
  // A plain fence and a non-cell language keep every line (a `#|` in prose code is not a header there).
  expect(copyText('bash', 'echo hi\n# comment')).toBe('echo hi\n# comment')
  expect(copyText('rust', '#| not a header\nfn main() {}')).toBe('#| not a header\nfn main() {}')
})

test('CRLF input copies with LF line ends, header dropped, trailing blank lines gone', () => {
  expect(copyText('bash', '#| cwd: /tmp\r\necho a\r\necho b\r\n\r\n  \r\n')).toBe('echo a\necho b')
  expect(copyText('bash', '$ ls\r\n$ pwd\r\n')).toBe('ls\npwd')
})

test('$ prompts go only when every non-blank line has one', () => {
  expect(copyText('bash', '$ cd /srv\n\n$ make\n')).toBe('cd /srv\n\nmake')
  expect(copyText('bash', '$ ls\nfile.txt')).toBe('$ ls\nfile.txt')
  expect(copyText('bash', '#| cwd: /tmp\n$ ls\n$ pwd')).toBe('ls\npwd')
  expect(copyText('bash', 'echo $ HOME')).toBe('echo $ HOME')
  // A kept shebang is a non-blank line without a prompt: the prompts stay (the rule is every non-blank line).
  expect(copyText('bash', '#!/bin/sh\n$ ls')).toBe('#!/bin/sh\n$ ls')
  expect(copyText('bash', '#!/bin/sh\n#| cwd: /tmp\n$ ls')).toBe('#!/bin/sh\n$ ls')
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

test('copyToClipboard writes the text and says Copied; a refused write says Copy failed', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })
  await expect(copyToClipboard('ls')).resolves.toBe(true)
  expect(writeText).toHaveBeenCalledWith('ls')
  expect(notify.message).toHaveBeenCalledWith('Copied')
  expect(notify.error).not.toHaveBeenCalled()

  vi.stubGlobal('navigator', { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) } })
  await expect(copyToClipboard('ls')).resolves.toBe(false)
  expect(notify.error).toHaveBeenCalledWith('Copy failed')

  // No clipboard API at all (an insecure context) fails the same way.
  vi.stubGlobal('navigator', {})
  await expect(copyToClipboard('ls')).resolves.toBe(false)
  expect(notify.error).toHaveBeenCalledTimes(2)
})
