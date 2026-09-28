import { createFileRoute } from '@tanstack/react-router'
import { PoppedCodeMap } from '@/components/codemap/CodeMapPanel'

/** One session's CodeMap on one root, full window (`lib/popout.ts` kind `codemap`, 73 §6.9). Under the `Popped` root when opened with `?window=`. */
export const Route = createFileRoute('/codemap/$session/$root')({
  component: () => {
    const { session, root } = Route.useParams()
    return <PoppedCodeMap session={session} root={root} />
  },
})
