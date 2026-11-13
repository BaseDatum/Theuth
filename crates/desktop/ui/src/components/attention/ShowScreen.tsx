import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ArtifactView } from '@/components/artifacts/Viewer'
import { metaLine } from '@/components/chat/ArtifactCard'
import { useAttentionActions, useMarkShown } from '@/components/hud/queries'
import { ARTIFACT_SIM, simFlag } from '@/components/settings/sandbox-queries'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { queryKeys, useApi } from '@/lib/rpc'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import { Body, useResolved } from './shared'
import { ActionBar, ContextLine, SnoozeButton, Source } from './source'

/** Being on the screen this long is seeing it (104 A2). */
const SEEN_AFTER_MS = 2_000

/**
 * 104 A6: an artifact an agent showed — Source · the context line · the artifact inline (`ArtifactView`, the record by
 * `call_id` as the Library fetches it) · name · kind · size · version · **Open in Library** · **Mark seen** (= dismiss) ·
 * Snooze. Two seconds on screen mark it seen by themselves; the item leaves the rail on the next refetch and the route
 * keeps this screen up until the user moves.
 */
export function ShowScreen({ item }: { item: AttentionItem }) {
  const api = useApi()
  const callId = item.id.replace(/^show:/, '')
  const record = useQuery({ queryKey: queryKeys.artifact(callId), queryFn: () => (simFlag() ? (ARTIFACT_SIM[callId] ?? Promise.reject(new Error('no such artifact in the sim'))) : api.get_artifact(callId)) })
  const act = useAttentionActions()
  const onResolved = useResolved(item)
  useMarkShown(callId, SEEN_AFTER_MS)
  const a = record.data
  return (
    <>
      <Body>
        <Source item={item} />
        <ContextLine session={item.session} beforeMs={item.at_ms} />
        <div className="flex flex-col gap-1">
          <h1 className="text-xl font-semibold leading-snug md:text-2xl">{a?.title ?? item.title}</h1>
          {a && (
            <p className="text-xs text-fg-tertiary">
              {metaLine(a)} · v{a.version}
            </p>
          )}
        </div>
        {record.isPending ? (
          <Skeleton className="h-64 w-full rounded-md" />
        ) : record.isError ? (
          <p className="text-sm text-danger">
            {record.error.message} —{' '}
            <Link to="/artifacts/$callId" params={{ callId }} className="underline underline-offset-2">
              try the Library
            </Link>
          </p>
        ) : (
          <div className="overflow-hidden rounded-md border">
            <ArtifactView artifact={record.data} className="max-h-[60vh] overflow-auto" />
          </div>
        )}
      </Body>
      <ActionBar>
        <Button asChild size="sm">
          <Link to="/artifacts/$callId" params={{ callId }}>
            Open in Library
          </Link>
        </Button>
        <Button size="sm" variant="outline" disabled={act.dismiss.isPending} onClick={() => void act.dismiss.mutateAsync(item.id).then(onResolved, () => undefined)}>
          Mark seen
        </Button>
        <SnoozeButton item={item} onResolved={onResolved} />
      </ActionBar>
    </>
  )
}
