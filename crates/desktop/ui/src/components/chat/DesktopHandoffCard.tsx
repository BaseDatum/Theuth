import { ExternalLink, Monitor } from 'lucide-react'
import { memo } from 'react'
import { useAttentionActions } from '@/components/hud/queries'
import { useHandoffs } from '@/components/settings/sandbox-queries'
import { Button } from '@/components/ui/button'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'

type Tool = Extract<Entry, { kind: 'tool' }>

type HandoffState = {
  title: string
  status: string
  waiting: boolean
}

function state(tool: Tool): HandoffState {
  if (!tool.result && !tool.interrupted) return { title: 'Your help is needed at the desktop', status: 'Waiting for you', waiting: true }
  const outcome = /^(done|cancelled|timeout|proceed_without)\b/.exec(tool.result?.output ?? '')?.[1]
  if (outcome === 'done') return { title: 'Desktop handoff completed', status: 'Done', waiting: false }
  if (outcome === 'cancelled') return { title: 'Desktop handoff cancelled', status: 'Cancelled', waiting: false }
  if (outcome === 'timeout') return { title: 'Desktop handoff timed out', status: 'Timed out', waiting: false }
  if (outcome === 'proceed_without') return { title: 'Desktop handoff left to the agent', status: 'Proceed without', waiting: false }
  return { title: 'Desktop handoff ended', status: tool.result?.isError ? 'Not completed' : 'Ended', waiting: false }
}

/** `desktop_handoff` stays in the transcript like the other human-input cards: the reason is an instruction to act,
 * not a tool argument to hide behind a disclosure. Done / Cancel resolve the same row as the attention item. */
export const DesktopHandoffCard = memo(function DesktopHandoffCard({ tool, session }: { tool: Tool; session: string }) {
  const args = typeof tool.args === 'object' && tool.args !== null ? (tool.args as Record<string, unknown>) : {}
  const reason = typeof args.reason === 'string' ? args.reason : ''
  const url = typeof args.url === 'string' && args.url ? args.url : null
  const current = state(tool)
  const handoff = useHandoffs().data?.find((h) => h.session === session && h.resolved_ms === null)
  const act = useAttentionActions()
  const resolve = (outcome: 'done' | 'cancelled') => {
    if (handoff) void act.handoff.mutateAsync({ id: handoff.id, outcome }).catch(() => undefined)
  }
  return (
    <section className="my-2 flex min-w-0 flex-col gap-3 rounded-xl border border-info-border bg-info-bg px-4 py-3 text-sm" aria-label={current.title}>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <Monitor className="size-4 shrink-0 text-info" aria-hidden="true" />
        <div className="font-medium text-info">{current.title}</div>
        <span className="ml-auto flex items-center gap-2 text-xs text-fg-secondary">
          {current.status}
          {current.waiting && <ToolSpinner />}
        </span>
      </div>
      {reason && <div className="wrap-anywhere whitespace-pre-wrap font-medium">{reason}</div>}
      {(url || current.waiting) && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          {url && (
            <Button asChild size="sm" variant="outline">
              <a href={url} target="_blank" rel="noreferrer">
                Open page
                <ExternalLink className="size-3.5" aria-hidden="true" />
              </a>
            </Button>
          )}
          {current.waiting && (
            <>
              <Button size="sm" disabled={!handoff || act.handoff.isPending} onClick={() => resolve('done')}>
                Done
              </Button>
              <Button size="sm" variant="outline" disabled={!handoff || act.handoff.isPending} onClick={() => resolve('cancelled')}>
                Cancel
              </Button>
            </>
          )}
        </div>
      )}
    </section>
  )
})
