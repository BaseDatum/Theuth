import type { Config } from '@react-router/dev/config'
import { glob } from 'node:fs/promises'
import { createGetUrl, getSlugs } from 'fumadocs-core/source'

const getUrl = createGetUrl('/docs')

/**
 * SPA mode (no server at runtime) plus static HTML for every docs page. The relay serves `dist/` as plain files with
 * `index.html` as the fallback for unknown paths; `/docs/**` deep links land on real HTML. `/` is prerendered as a
 * shell only (routes/home.tsx mounts the marketing page after hydration) so its module preloads are in the HTML.
 */
export default {
  appDirectory: 'src',
  ssr: false,
  async prerender() {
    const paths = ['/', '/api/search']
    for await (const entry of glob('**/*.mdx', { cwd: 'content/docs' })) paths.push(getUrl(getSlugs(entry)))
    return paths
  },
} satisfies Config
