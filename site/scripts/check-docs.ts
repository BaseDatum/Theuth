// Validates content/docs before the build: frontmatter, slugs, meta.json entries and every internal /docs link
// (in MDX and in src/**/*.tsx). Offline and deterministic; exits 1 with one line per problem.
import { existsSync, readFileSync } from 'node:fs'
import { glob } from 'node:fs/promises'
import { dirname, join, relative } from 'node:path'
import { createGetUrl, getSlugs } from 'fumadocs-core/source'

const ROOT = new URL('..', import.meta.url).pathname
const DOCS = join(ROOT, 'content/docs')
const getUrl = createGetUrl('/docs')
const problems: string[] = []

const files = await Array.fromAsync(glob('**/*.mdx', { cwd: DOCS }))
const urls = new Set(files.map((f) => getUrl(getSlugs(f))))

for (const file of files) {
  const text = readFileSync(join(DOCS, file), 'utf8')
  const fm = /^---\r?\n([\s\S]*?)\r?\n---/.exec(text)?.[1]
  if (!fm) problems.push(`${file}: missing frontmatter`)
  else {
    if (!/^title:\s*\S/m.test(fm)) problems.push(`${file}: frontmatter has no title`)
    if (!/^description:\s*\S/m.test(fm)) problems.push(`${file}: frontmatter has no description`)
  }
  for (const seg of file.replace(/\.mdx$/, '').split('/')) {
    if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(seg)) problems.push(`${file}: "${seg}" is not kebab-case`)
  }
}

for (const file of await Array.fromAsync(glob('**/meta.json', { cwd: DOCS }))) {
  const dir = join(DOCS, dirname(file))
  let meta: { pages?: unknown }
  try {
    meta = JSON.parse(readFileSync(join(DOCS, file), 'utf8')) as { pages?: unknown }
  } catch (e) {
    problems.push(`${file}: ${(e as Error).message}`)
    continue
  }
  if (meta.pages === undefined) continue
  if (!Array.isArray(meta.pages)) {
    problems.push(`${file}: "pages" must be an array`)
    continue
  }
  for (const entry of meta.pages as unknown[]) {
    if (typeof entry !== 'string') {
      problems.push(`${file}: non-string entry ${JSON.stringify(entry)}`)
      continue
    }
    if (entry.startsWith('...') || entry.startsWith('---') || entry.startsWith('[')) continue
    const name = entry.replace(/^!/, '')
    if (!existsSync(join(dir, `${name}.mdx`)) && !existsSync(join(dir, name)))
      problems.push(`${file}: "${entry}" matches no page or folder`)
  }
}

// Internal design notes are not public: no links, paths or quotes pointing at them from the published docs.
const PRIVATE = /docs\/design\b/
const LINK = /(?:\]\(|href=["'])(\/docs(?:\/[^)"'\s#?]*)?)/g
const sources = [
  ...files.map((f) => join(DOCS, f)),
  ...(await Array.fromAsync(glob('src/**/*.tsx', { cwd: ROOT }))).map((f) => join(ROOT, f)),
]
for (const path of sources) {
  const text = readFileSync(path, 'utf8')
  if (PRIVATE.test(text)) problems.push(`${relative(ROOT, path)}: references docs/design (internal)`)
  for (const m of text.matchAll(LINK)) {
    const target = (m[1] ?? '').replace(/\/$/, '') || '/docs'
    if (!urls.has(target)) problems.push(`${relative(ROOT, path)}: broken link ${target}`)
  }
}

if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log(`docs ok: ${files.length} pages`)
