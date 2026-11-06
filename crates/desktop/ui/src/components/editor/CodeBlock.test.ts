import { expect, test } from 'vitest'
import { codeBlock } from './CodeBlock'

test('meta.highlight: the shiki language BlockNote loads, cell forms through the alias map, plain text skipped', () => {
  const highlight = (language: string) => codeBlock.implementation.meta?.highlight?.({ type: 'codeBlock', props: { language } } as never)
  expect(highlight('pwsh')).toBe('powershell')
  expect(highlight('shellscript')).toBe('shellscript')
  expect(highlight('py')).toBe('python')
  expect(highlight('rust')).toBe('rust')
  expect(highlight('ansi')).toBe('text')
  expect(highlight('constructor')).toBe('constructor')
})
