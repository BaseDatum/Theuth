import { createFileRoute } from '@tanstack/react-router'
import { PoppedFile } from '@/components/code/Popped'
import { lineParam } from '@/lib/popout'

type Search = { path: string; line?: number; end_line?: number }

/** One `show_file` file, full window (`lib/popout.ts` kind `file`, 212), following the disk. Under the `Popped` root
 * when opened with `?window=`. */
export const Route = createFileRoute('/file/$session')({
  // The router reads search values as JSON where they parse (`line=12` is a number, `path=42` too): normalise.
  validateSearch: (s: Record<string, unknown>): Search => {
    const line = lineParam(s.line), end = lineParam(s.end_line)
    return { path: s.path === undefined ? '' : String(s.path), ...(line === undefined ? {} : { line }), ...(end === undefined ? {} : { end_line: end }) }
  },
  component: FileRoute,
})

function FileRoute() {
  const { session } = Route.useParams()
  const { path, line, end_line } = Route.useSearch()
  if (!path) return <p className="p-3 text-sm text-fg-tertiary">No file named.</p>
  return <PoppedFile session={session} path={path} line={line} end={end_line} />
}
