import defaultMdxComponents from 'fumadocs-ui/mdx'

/** Fumadocs' defaults: shiki `pre` with copy button, Card(s), Callout, heading anchors, framework-aware `a`, `img`. */
export const mdxComponents = defaultMdxComponents

declare global {
  type MDXProvidedComponents = typeof mdxComponents
}
