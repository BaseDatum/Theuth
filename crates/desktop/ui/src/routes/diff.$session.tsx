import { createFileRoute } from '@tanstack/react-router'
import { PoppedDiff } from '@/components/code/Popped'
import { specParam } from '@/lib/popout'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'

type Search = { spec: DiffSpec | null }

/** One `show_diff` diff, full window (`lib/popout.ts` kind `diff`, 212). Under the `Popped` root when opened with
 * `?window=`. */
export const Route = createFileRoute('/diff/$session')({
  // `spec` is the DiffSpec's JSON; the router has usually parsed it into the object already.
  validateSearch: (s: Record<string, unknown>): Search => ({ spec: specParam(s.spec) }),
  component: DiffRoute,
})

function DiffRoute() {
  const { session } = Route.useParams()
  const { spec } = Route.useSearch()
  if (!spec) return <p className="p-3 text-sm text-fg-tertiary">No diff named.</p>
  return <PoppedDiff session={session} spec={spec} />
}
