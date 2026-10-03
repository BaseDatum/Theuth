import { Link } from 'react-router'

/** Static hosting answers every path with 200 + index.html, so this is the only 404 a visitor sees; keep it out of indexes. */
export function NotFound() {
  return (
    <main className="mx-auto flex min-h-[60vh] w-full max-w-2xl flex-col justify-center px-6 py-24">
      <meta name="robots" content="noindex" />
      <p className="index">404</p>
      <h1 className="display mt-4 text-5xl">Not found</h1>
      <p className="mt-6 text-fg-2">There is no page at this address.</p>
      <p className="mt-10 flex gap-3">
        <Link className="btn" to="/">
          Home
        </Link>
        <Link className="btn btn-primary" to="/docs">
          Docs
        </Link>
      </p>
    </main>
  )
}
