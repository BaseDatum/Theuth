import type { InferPageType } from 'fumadocs-core/source'
import { DocsLayout } from 'fumadocs-ui/layouts/docs'
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/layouts/docs/page'
import { use } from 'react'
import { type ClientLoaderFunctionArgs, type LoaderFunctionArgs, type MetaFunction, useParams } from 'react-router'
import { mdxComponents } from '../components/mdx'
import { NotFound } from '../components/not-found'
import { SITE, baseOptions } from '../lib/layout.shared'
import { source } from '../lib/source'

type Page = InferPageType<typeof source>

/** `/docs/a/b/` and `/docs/a/b` are the same page; the relay redirects directory URLs to the slash form. */
const pageFor = (splat: string | undefined) => source.getPage((splat ?? '').split('/').filter(Boolean))

export const meta: MetaFunction = ({ params }) => {
  const page = pageFor(params['*'])
  if (!page) return [{ title: 'Not found · theuth docs' }, { name: 'robots', content: 'noindex' }]
  const title = page.slugs.length === 0 ? 'theuth docs' : `${page.data.title} · theuth docs`
  const url = SITE + page.url
  return [
    { title },
    { name: 'description', content: page.data.description },
    { tagName: 'link', rel: 'canonical', href: url },
    { property: 'og:type', content: 'article' },
    { property: 'og:title', content: title },
    { property: 'og:description', content: page.data.description },
    { property: 'og:url', content: url },
    { property: 'og:image', content: `${SITE}/og.jpg` },
    { name: 'twitter:card', content: 'summary' },
  ]
}

/**
 * Both loaders only warm the page's body chunk so render never suspends: at prerender the HTML carries the article
 * inline (a suspended boundary would be streamed as a hidden segment); on client navigation RR awaits `clientLoader`
 * and never fetches `.data` files, which the plain static server could not answer for unknown pages anyway.
 */
export async function loader({ params }: LoaderFunctionArgs) {
  await pageFor(params['*'])?.data.preload()
  return null
}
export async function clientLoader({ params }: ClientLoaderFunctionArgs) {
  await pageFor(params['*'])?.data.preload()
  return null
}

export default function Docs() {
  const page = pageFor(useParams()['*'])
  return (
    <DocsLayout {...baseOptions} tree={source.pageTree}>
      {page ? <Content page={page} /> : <NotFound />}
    </DocsLayout>
  )
}

function Content({ page }: { page: Page }) {
  // Preloaded by the loaders; `load()` is memoised and status-tracked, so `use` unwraps without suspending.
  const { toc } = use(page.data.load())
  const Mdx = page.data.body
  return (
    <DocsPage toc={toc}>
      <DocsTitle>{page.data.title}</DocsTitle>
      <DocsDescription>{page.data.description}</DocsDescription>
      <DocsBody>
        <Mdx components={mdxComponents} />
      </DocsBody>
    </DocsPage>
  )
}
