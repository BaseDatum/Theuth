import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { memo, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { alwaysAskWhyText, setAlwaysAsk, useInvalidateAlwaysAsk } from '@/lib/always-ask'
import { chatName } from '@/lib/attach'
import { describe } from '@/lib/describe'
import { useEngineLabel } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import type { ApprovalRequested } from "@/rpc/bindings/event.rpc_2estream.output"
import type { Decision } from "@/rpc/bindings/event.tools_2eapproval_2dresolved.output"
import type { Risk } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { useApprovals } from '@/store/approvals'
import { DoubleCard, DoubleChip, preselect, useDoubleCard } from './DoubleChip'
import { SharePublishApproval } from '@/components/public-share/SharePublishApproval'

const RISK: Record<Risk, string> = {
  read_only: 'read-only',
  mutating: 'mutating',
  destructive: 'destructive',
}

/** 183 W-6: the owner's `approval_timeout_secs` default; the event carries no deadline, so the card counts from when it
 * arrived live (`store/approvals` `arrived`; 183 R1: a snapshot or refocus replay has no arrival, so no deadline).
 * ponytail: assumes the owner kept the 300 s default; an engine-sent deadline replaces this. */
const OWNER_TIMEOUT_MS = 300_000

/** §12.3 / mock panel 3: `mutating · relayed from cloud-1 · denies at 11:47` under a card whose owner is elsewhere. */
export function relayedLine(risk: Risk, owner: string, deadline: number | null): string {
  if (deadline === null) return `${RISK[risk]} · relayed from ${owner}`
  const at = new Date(deadline).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  return `${RISK[risk]} · relayed from ${owner} · denies at ${at}`
}

const NIL_SESSION = '00000000-0000-0000-0000-000000000000'

/** 219 §6.1: whether a non-always-ask card offers "Always ask for <tool>". Not on a human-only card (already the human's),
 * not without a chat to mark (the nil session), not on the removal card itself, and not on an ACP title (`rm -rf build`)
 * that no registry entry can match. */
export function offersAlwaysAsk(approval: ApprovalRequested): boolean {
  return !approval.always_ask && !approval.human_only && approval.session !== NIL_SESSION
    && approval.tool !== 'always_ask.remove' && !/\s/.test(approval.tool)
}

/** `approval: tool (risk)`, the described arguments, Allow / Deny. Rendered at the tool row or at the transcript's end.
 * 126 §10: the double's chip beside the title; at L1 its choice is the filled button; at L2 the whole card takes over.
 * 183 §12.3: a session owned elsewhere reads `<owner> asks: tool` with the relayed line; the answer forwards.
 * 219 §6.1: an always-ask call is the human's alone (an `always ask` badge, no double chip); any other card offers to
 * make its tool always ask in this chat. */
export const ApprovalCard = memo(function ApprovalCard({
  approval,
}: {
  approval: ApprovalRequested
}) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const api = useApi()
  // The card's session decides: owner ≠ this engine → relayed (`list_sessions` is the sidebar's warm cache).
  const sessions = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const row = sessions.data?.find((s) => s.id === approval.session)
  const owner = useEngineLabel(row?.owner ?? '')
  const relayed = !!row && !row.here
  // Read, not subscribed: the arrival is set before the card exists and only pruned when it goes.
  const arrived = useApprovals.getState().arrived.get(approval.id)
  const deadline = arrived === undefined ? null : arrived + OWNER_TIMEOUT_MS
  const [busy, setBusy] = useState(false)
  const [marked, setMarked] = useState(false)
  const canMark = useQuerySupport(['set_always_ask'])
  const refetchAlwaysAsk = useInvalidateAlwaysAsk()
  const double = useDoubleCard([approval.id])
  const always = approval.always_ask === true
  // L1+: the double's pick is the filled button (the model has no `recommended` on an approval); it never answers always-ask.
  const pick = always ? null : preselect(double.prediction, ['Allow', 'Deny'])
  const decide = async (decision: Decision) => {
    setBusy(true)
    try {
      if (!(await api.resolve_approval(approval.id, decision))) {
        // Decided elsewhere and the `ApprovalResolved` was missed: the tool row carries the outcome.
        useApprovals.getState().resolve(approval.id)
        toast.error('the approval was already decided')
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  // Adding is never gated (219 §2.4), so no confirm; only this chat's scope is offered here.
  const mark = async () => {
    try {
      await setAlwaysAsk(api, approval.session, approval.tool, true)
      setMarked(true)
      void refetchAlwaysAsk()
      toast(`${approval.tool} always asks in this chat`)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  // 210 §7.3: human-only, so no double chip; the share layout replaces the generic argument dump.
  if (approval.tool === 'share_publish') return <SharePublishApproval args={approval.args} busy={busy} decide={(d) => void decide(d)} />
  return (
    <DoubleCard ids={[approval.id]} anchor={approval.id} className="my-2 rounded-xl border border-warning-border bg-warning-bg px-4 py-3 text-sm">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <div className="font-medium text-warning">
          {relayed ? `${owner} asks: ${approval.tool}` : `approval: ${approval.tool} (${RISK[approval.risk]})`}
        </div>
        {always ? (
          <Badge variant="outline" className="font-normal text-warning">
            always ask
          </Badge>
        ) : (
          <DoubleChip ids={[approval.id]} />
        )}
      </div>
      {relayed && <div className="mt-0.5 text-xs text-fg-secondary">{relayedLine(approval.risk, owner, deadline)}</div>}
      {always && approval.always_ask_why && (
        <div data-always-ask-why="" className="mt-0.5 text-xs text-fg-secondary">
          {alwaysAskWhyText(approval.always_ask_why, approval.tool)}
        </div>
      )}
      <pre className="mt-1 max-w-full overflow-x-auto font-mono text-xs leading-5 text-fg-secondary whitespace-pre-wrap">
        {describe(approval.tool, approval.args).join('\n')}
      </pre>
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button
          variant={pick === 1 ? 'outline' : 'default'}
          size="sm"
          className="pointer-coarse:h-11 pointer-coarse:flex-1"
          disabled={busy}
          onClick={() => decide('allow')}
        >
          Allow
        </Button>
        <Button
          size="sm"
          variant={pick === 1 ? 'default' : 'outline'}
          className="pointer-coarse:h-11 pointer-coarse:flex-1"
          disabled={busy}
          onClick={() => decide('deny')}
        >
          Deny
        </Button>
        {offersAlwaysAsk(approval) && canMark && !marked && (
          <Button variant="ghost" size="sm" className="ml-auto text-fg-secondary pointer-coarse:h-11" onClick={() => void mark()}>
            {`Always ask for ${approval.tool}`}
          </Button>
        )}
      </div>
    </DoubleCard>
  )
})

/** Every pending approval across sessions, each with a link to its session. */
export function ApprovalsSheet({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange(open: boolean): void
}) {
  const pending = useApprovals((s) => s.pending)
  // The sidebar keeps `list_sessions` warm; a chat the cache lacks is just "chat", never an id.
  const sessions = useQueryClient().getQueryData<SessionSummary[]>(queryKeys.sessions)
  const name = (id: string) => {
    const s = sessions?.find((x) => x.id === id)
    return s ? chatName(s) : 'chat'
  }
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full overflow-y-auto sm:w-[480px] sm:max-w-[480px]">
        <SheetHeader>
          <SheetTitle>Pending approvals</SheetTitle>
          <SheetDescription>
            {pending.length === 0 ? 'Nothing is waiting.' : `${pending.length} waiting`}
          </SheetDescription>
        </SheetHeader>
        <div className="flex flex-col gap-4 px-4 pb-4">
          {pending.map((p) => (
            <div key={p.id}>
              <Link
                to="/c/$session"
                params={{ session: p.session }}
                onClick={() => onOpenChange(false)}
                className="text-xs text-fg-accent hover:underline"
              >
                open {name(p.session)}
              </Link>
              <ApprovalCard approval={p} />
            </div>
          ))}
        </div>
      </SheetContent>
    </Sheet>
  )
}
