import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/attention/requirements'
import { Link, createFileRoute } from '@tanstack/react-router'
import { useRef } from 'react'
import { Screen } from '@/components/attention/Screen'
import { useAttention } from '@/components/hud/queries'
import { Skeleton } from '@/components/ui/skeleton'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"

/**
 * `/attention/$id` (101 D6): the selected item's screen; the router decodes the `:`-bearing id. 104 A2: an item that
 * leaves the list while its screen is open (a show marked seen by being looked at) stays on screen until the user moves.
 */
function Selected() {
  const { id } = Route.useParams()
  const attention = useAttention()
  const held = useRef<AttentionItem | null>(null)
  const live = attention.data?.find((i) => i.id === id)
  if (live) held.current = live
  const item = live ?? (held.current?.id === id ? held.current : undefined)
  if (item) return <Screen item={item} />
  if (attention.isPending) {
    return (
      <div className="mx-auto w-full max-w-[720px] space-y-3 p-6">
        <Skeleton className="h-8 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    )
  }
  return (
    <div className="m-auto flex flex-col items-center gap-2 p-6 text-center text-sm text-fg-tertiary">
      <p>{attention.isError ? attention.error.message : 'This item is no longer in the inbox.'}</p>
      <Link to="/attention" search={true} className="underline underline-offset-2">
        Back to inbox
      </Link>
    </div>
  )
}

export const Route = createFileRoute('/attention/$id')({ component: withCapability(Selected, requirements) })
