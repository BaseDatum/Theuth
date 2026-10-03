import { loader } from 'fumadocs-core/source'
import { defineDocs } from 'fumadocs-mdx/macro'

/** content/docs, compiled by fumadocs-mdx's Vite plugin. `async`: frontmatter is bundled, bodies load per page. */
export const docs = defineDocs({
  dir: 'content/docs',
  docs: { async: true },
})

export const source = loader({
  baseUrl: '/docs',
  source: docs.toFumadocsSource(),
})
