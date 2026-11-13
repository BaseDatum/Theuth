import { DoubleChip, preselect, useDoubleCard } from '@/components/chat/DoubleChip'
import { useAttentionActions } from '@/components/hud/queries'
import { Button } from '@/components/ui/button'
import { describe } from '@/lib/describe'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import type { Risk } from "@/rpc/bindings/method.agent.output"
import { useApprovals } from '@/store/approvals'
import { Body, Footer, SectionTitle, useResolved } from './shared'
import { ActionBar, ContextLine, Source } from './source'

const RISK: Record<Risk, string> = { read_only: 'read-only', mutating: 'mutating', destructive: 'destructive' }

/**
 * 104 A6: a tool call waiting on you — Source · the context line · the tool as the heading · a `command` as a code
 * block, every other argument as `key: value` · **Allow** / **Deny** large · Snooze / Dismiss. The full arguments come
 * from the engine's pending-prompts snapshot (`useApprovals`, every session); when this client has not got that
 * request, the item's `detail` (the command) is what shows, with a note.
 */
export function ApprovalScreen({ item }: { item: AttentionItem }) {
  const act = useAttentionActions()
  const onResolved = useResolved(item)
  const request = useApprovals((s) => s.pending.find((p) => p.id === item.approval))
  const tool = request?.tool ?? item.title.replace(/^.* wants to run /, '')
  const args = typeof request?.args === 'object' && request.args !== null && !Array.isArray(request.args) ? (request.args as Record<string, unknown>) : null
  const command = typeof args?.command === 'string' ? args.command : item.detail
  const rest = args ? describe(tool, Object.fromEntries(Object.entries(args).filter(([k]) => k !== 'command'))) : []
  const busy = act.approve.isPending
  // 126 §10: the double's chip under the heading; at L1 its pick is the filled button.
  const double = useDoubleCard([item.approval])
  const pick = preselect(double.prediction, ['Allow', 'Deny'])
  const decide = (allow: boolean) => {
    if (!item.approval) return
    void act.approve.mutateAsync({ id: item.approval, allow }).then(onResolved, () => undefined)
  }
  return (
    <>
      <Body>
        <Source item={item} />
        <ContextLine session={item.session} beforeMs={item.at_ms} />
        <div className="flex flex-col gap-1">
          <h1 className="font-mono text-xl font-semibold leading-snug md:text-2xl">{tool}</h1>
          <p className="text-sm text-fg-secondary">wants to run{request ? ` · ${RISK[request.risk]}` : ''}</p>
          <DoubleChip ids={[item.approval]} className="mt-1 w-fit" />
        </div>
        {command && (
          <div className="flex flex-col gap-1.5">
            <SectionTitle>Command</SectionTitle>
            <pre className="overflow-x-auto rounded-md border border-border-subtle bg-layer-1 px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap">{command}</pre>
          </div>
        )}
        {rest.length > 0 && (
          <div className="flex flex-col gap-1.5">
            <SectionTitle>Arguments</SectionTitle>
            <pre className="overflow-x-auto rounded-md border border-border-subtle bg-layer-1 px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap">{rest.join('\n')}</pre>
          </div>
        )}
        {!request && <p className="text-xs text-fg-tertiary">{command ? 'Only the command reached this inbox; the full arguments are in the session.' : 'The arguments are in the session.'}</p>}
      </Body>
      <ActionBar>
        <Button size="default" variant={pick === 1 ? 'outline' : 'default'} disabled={busy || !item.approval} onClick={() => decide(true)}>
          Allow
        </Button>
        <Button size="default" variant={pick === 1 ? 'default' : 'outline'} disabled={busy || !item.approval} onClick={() => decide(false)}>
          Deny
        </Button>
        <Footer item={item} onResolved={onResolved} />
      </ActionBar>
    </>
  )
}
