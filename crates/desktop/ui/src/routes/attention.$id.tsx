import { Link, createFileRoute } from '@tanstack/react-router'
import { Screen } from '@/components/attention/Screen'
import { useAttention } from '@/components/hud/queries'
import { Skeleton } from '@/components/ui/skeleton'

/** `/attention/$id` (101 D6): the selected item's screen; the router decodes the `:`-bearing id. */
function Selected() {
  const { id } = Route.useParams()
  const attention = useAttention()
  const item = attention.data?.find((i) => i.id === id)
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
        Back to attention
      </Link>
    </div>
  )
}

export const Route = createFileRoute('/attention/$id')({ component: Selected })
