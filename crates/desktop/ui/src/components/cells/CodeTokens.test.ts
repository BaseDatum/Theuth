import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { tokensOf } from '@/lib/shiki'
import { CodeTokensView, type Lit } from './CodeTokens'

const CODE = '#| label: x\necho "hi"'
const html = (lang: string, lit: Lit | null) => renderToStaticMarkup(createElement(CodeTokensView, { code: CODE, lang, lit }))

test('CodeTokens: tokens for this code and lang render as shiki spans; another lang or code renders plain', async () => {
  const lit: Lit = { code: CODE, lang: 'bash', tokens: await tokensOf(CODE, 'bash') }
  const lit_ = html('bash', lit)
  expect(lit_).toContain('<span class="shiki" style="--shiki-light:#6A737D;--shiki-dark:#6A737D">#| label: x</span>\n')
  expect(lit_.replace(/<[^>]+>/g, '')).toBe(CODE.replace(/"/g, '&quot;'))
  // A lang change (or new code) shows the text plain until its own tokens arrive, never the old colours.
  expect(html('python', lit)).toBe(CODE.replace(/"/g, '&quot;'))
  expect(renderToStaticMarkup(createElement(CodeTokensView, { code: `${CODE}\n`, lang: 'bash', lit }))).not.toContain('shiki')
  expect(html('bash', null)).not.toContain('shiki')
})
