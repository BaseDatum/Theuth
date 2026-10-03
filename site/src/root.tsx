import './docs.css'
import { RootProvider } from 'fumadocs-ui/provider/react-router'
import { isRouteErrorResponse, Links, Meta, type MetaFunction, Outlet, Scripts, ScrollRestoration } from 'react-router'
import { lazy, type ReactNode } from 'react'
import { NotFound } from './components/not-found'

/** Loaded on the first ⌘K, not with the homepage: the search client and index builder are ~400 KB of JS. */
const SearchDialog = lazy(() => import('./components/search'))

export const meta: MetaFunction = () => [
  { title: 'theuth: the harness that does things' },
  { name: 'description', content: 'A single-process AI harness that does things for the user.' },
  { property: 'og:title', content: 'theuth' },
  { property: 'og:description', content: 'A single-process AI harness that does things for the user.' },
  { property: 'og:image', content: 'https://theuth.io/og.jpg' },
  { property: 'og:url', content: 'https://theuth.io' },
  { name: 'twitter:card', content: 'summary' },
]

/**
 * Mirrors the OS colour scheme onto `html.dark` before first paint: styles.css tokens already follow
 * `prefers-color-scheme`, Fumadocs' `dark:` utilities and shiki's dual themes key off the class.
 */
const THEME = `(m=>{const s=()=>document.documentElement.classList.toggle('dark',m.matches);s();m.addEventListener('change',s)})(matchMedia('(prefers-color-scheme: dark)'))`

export function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <meta charSet="UTF-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
        <meta name="color-scheme" content="dark light" />
        <link rel="icon" href="/icon.svg" type="image/svg+xml" />
        <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
        <Meta />
        <Links />
        <script dangerouslySetInnerHTML={{ __html: THEME }} />
      </head>
      <body>
        <RootProvider theme={{ enabled: false }} search={{ SearchDialog, preload: false }}>
          {children}
        </RootProvider>
        <ScrollRestoration />
        <Scripts />
      </body>
    </html>
  )
}

export default function Root() {
  return <Outlet />
}

export function ErrorBoundary({ error }: { error: unknown }) {
  if (isRouteErrorResponse(error) && error.status === 404) return <NotFound />
  const detail = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : 'Something went wrong.'
  return (
    <main className="mx-auto flex min-h-screen w-full max-w-2xl flex-col justify-center px-6 py-24">
      <p className="index">error</p>
      <h1 className="display mt-4 text-5xl">Something broke</h1>
      <p className="mt-6 text-fg-2">{detail}</p>
      <p className="mt-10 flex gap-3">
        <a className="btn" href="/">
          Home
        </a>
        <a className="btn" href="/docs">
          Docs
        </a>
      </p>
    </main>
  )
}
