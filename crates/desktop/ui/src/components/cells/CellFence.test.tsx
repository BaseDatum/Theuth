import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { CodeBlock } from 'streamdown'
import { expect, test } from 'vitest'
import { CELL_LANGUAGES, CellFence } from './CellFence'

const fence = (language: string, code: string) => renderToStaticMarkup(createElement(CellFence, { code, language, isIncomplete: false }))

test('a fence without a #| header is streamdown’s own code block, byte for byte (171 §4.1)', () => {
  for (const [language, code] of [
    ['bash', 'echo hi\nls -la'],
    ['python', 'print(1)'],
  ] as const) {
    const html = fence(language, code)
    expect(html).toBe(renderToStaticMarkup(createElement(CodeBlock, { code, language, isIncomplete: false })))
    expect(html).toContain('data-streamdown="code-block"')
    expect(html).toContain(`data-language="${language}"`)
    expect(html).toContain('<pre')
    for (const line of code.split('\n')) expect(html).toContain(line)
    expect(html).not.toMatch(/data-streamdown="cell|data-cell/)
  }
})

test('the renderer claims exactly the shell and python fence languages', () => {
  expect([...CELL_LANGUAGES].sort()).toEqual(['bash', 'powershell', 'pwsh', 'py', 'python', 'sh', 'zsh'])
})
