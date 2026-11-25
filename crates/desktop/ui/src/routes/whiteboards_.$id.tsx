import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/whiteboard/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { WhiteboardView } from '@/components/whiteboard/WhiteboardView'

/** `/whiteboards/$id` (218): one whiteboard; `$id` is the DocId, `?session` the chat bound beside it. */
export const Route = createFileRoute('/whiteboards_/$id')({
  validateSearch: (s: Record<string, unknown>): { session?: string } => ({ session: typeof s.session === 'string' ? s.session : undefined }),
  component: withCapability(Page, requirements),
})

function Page() {
  const { id } = Route.useParams()
  const { session } = Route.useSearch()
  const navigate = Route.useNavigate()
  return (
    <WhiteboardView
      doc={id}
      session={session ?? null}
      onSession={(s: string | null) => void navigate({ search: { session: s ?? undefined }, replace: true })}
      onClose={() => void navigate({ to: '/whiteboards' })}
    />
  )
}
