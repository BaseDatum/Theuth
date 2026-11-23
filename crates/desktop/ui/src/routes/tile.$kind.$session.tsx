import { createFileRoute } from '@tanstack/react-router'
import { PoppedTile } from '@/components/tiles/registry'
import { tileProps } from '@/lib/popout'

type Search = { props?: unknown; stash?: string }

/** One generic tile, full window (`lib/popout.ts` kind `tile`): its kind's own renderer over its props. Under the
 * `Popped` root when opened with `?window=`. */
export const Route = createFileRoute('/tile/$kind/$session')({
  // The router reads search values as JSON where they parse: `props` may arrive as the object, `stash` as a number.
  validateSearch: (s: Record<string, unknown>): Search => ({
    ...(s.props === undefined ? {} : { props: s.props }),
    ...(s.stash === undefined ? {} : { stash: String(s.stash) }),
  }),
  component: TileRoute,
})

function TileRoute() {
  const { kind, session } = Route.useParams()
  const search = Route.useSearch()
  return <PoppedTile kind={kind} session={session} props={tileProps(search)} />
}
