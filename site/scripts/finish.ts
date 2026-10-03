// After `react-router build`: build/client → dist (the relay serves dist/ as plain files), sitemap.xml, and a check that
// every docs page has crawlable HTML, the search index parses, and the SPA shell exists.
import { existsSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { glob } from 'node:fs/promises'
import { join } from 'node:path'
import { createGetUrl, getSlugs } from 'fumadocs-core/source'

const ROOT = new URL('..', import.meta.url).pathname
const DIST = join(ROOT, 'dist')
const SITE = 'https://theuth.io'

rmSync(DIST, { recursive: true, force: true })
renameSync(join(ROOT, 'build/client'), DIST)
rmSync(join(ROOT, 'build'), { recursive: true, force: true })
// The relay's fallback is index.html (the prerendered `/` shell); RR's extra copy is dead weight.
rmSync(join(DIST, '__spa-fallback.html'), { force: true })

const getUrl = createGetUrl('/docs')
const files = await Array.fromAsync(glob('**/*.mdx', { cwd: join(ROOT, 'content/docs') }))
const urls = files.map((f) => getUrl(getSlugs(f))).sort()

writeFileSync(
  join(DIST, 'sitemap.xml'),
  `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${['/', ...urls]
    .map((u) => `  <url><loc>${SITE}${u}</loc></url>`)
    .join('\n')}\n</urlset>\n`,
)

const problems: string[] = []
if (!existsSync(join(DIST, 'index.html'))) problems.push('dist/index.html (SPA shell) missing')
for (const url of urls) {
  const file = join(DIST, url, 'index.html')
  if (!existsSync(file)) {
    problems.push(`${url}: no prerendered HTML`)
    continue
  }
  const html = readFileSync(file, 'utf8')
  if (!/<title>[^<]+<\/title>/.test(html)) problems.push(`${url}: no <title>`)
  if (!html.includes(`<link rel="canonical" href="${SITE}${url}"`)) problems.push(`${url}: no canonical`)
  if (!html.includes('<article')) problems.push(`${url}: no article body`)
}
try {
  const records = JSON.parse(readFileSync(join(DIST, 'api/search'), 'utf8')) as { url?: string }[]
  if (records.length !== urls.length) problems.push(`dist/api/search: ${records.length} records for ${urls.length} pages`)
} catch (e) {
  problems.push(`dist/api/search: ${(e as Error).message}`)
}
if (problems.length) {
  console.error(problems.join('\n'))
  process.exit(1)
}
console.log(`dist ok: ${urls.length} docs pages, sitemap.xml, api/search`)
