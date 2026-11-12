import { expect, test } from 'vitest'
import type { Lang } from '@/rpc/bindings/auxiliary.surface.output'
import { loadHighlighter, shikiLang } from './shiki'

test('shikiLang: every cell Lang and every stored form resolves to a shiki grammar; other languages pass through', async () => {
  // `Record<Lang, …>`: a new Lang fails to compile here until it is mapped.
  const langs: Record<Lang, string> = { bash: 'shellscript', zsh: 'shellscript', sh: 'shellscript', pwsh: 'powershell', python: 'python', javascript: 'javascript', typescript: 'typescript' }
  const stored = { shellscript: 'shellscript', shell: 'shellscript', powershell: 'powershell', ps1: 'powershell', py: 'python', js: 'javascript', ts: 'typescript', ' Bash ': 'shellscript' }
  const h = await loadHighlighter()
  for (const [form, id] of Object.entries({ ...langs, ...stored })) {
    expect(shikiLang(form), form).toBe(id)
    await h.loadLanguage(shikiLang(form) as never)
    expect(h.getLoadedLanguages(), form).toContain(id)
  }
  expect(shikiLang('rust')).toBe('rust')
  expect(shikiLang('json-render')).toBe('json-render')
  // Object prototype names are languages, not map hits; missing is empty; plain-text ids are BlockNote's skipped `text`.
  for (const l of ['constructor', 'toString', '__proto__']) expect(shikiLang(l)).toBe(l.toLowerCase())
  expect(shikiLang(undefined)).toBe('')
  for (const l of ['ansi', 'plain', 'plaintext', 'txt', 'TEXT']) expect(shikiLang(l)).toBe('text')
})

test('an unknown language rejects instead of throwing: BlockNote catches the rejection, a throw unhighlights the whole note', async () => {
  const h = await loadHighlighter()
  let thrown: unknown = null
  let p: Promise<void> | undefined
  try {
    p = h.loadLanguage('cells' as never)
  } catch (e) {
    thrown = e
  }
  expect(thrown).toBeNull()
  await expect(p).rejects.toThrow(/cells/)
})
