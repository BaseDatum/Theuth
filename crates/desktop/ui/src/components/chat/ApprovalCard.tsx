import { Link } from '@tanstack/react-router'
import { memo, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { describe } from '@/lib/describe'
import { useApi } from '@/lib/rpc'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { Decision } from '@/rpc/bindings/Decision'
import type { Risk } from '@/rpc/bindings/Risk'
import { useApprovals } from '@/store/approvals'

const RISK: Record<Risk, string> = {
  read_only: 'read-only',
  mutating: 'mutating',
  destructive: 'destructive',
}

/** `approval: tool (risk)`, the described arguments, Allow / Deny. Rendered at the tool row or at the transcript's end. */
export const ApprovalCard = memo(function ApprovalCard({
  approval,
}: {
  approval: ApprovalRequested
}) {
  const api = useApi()
  const [busy, setBusy] = useState(false)
  const decide = async (decision: Decision) => {
    setBusy(true)
    try {
      if (!(await api.resolve_approval(approval.id, decision)))
        toast.error('approval was no longer pending')
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="my-2 rounded-xl border border-warning-border bg-warning-bg px-4 py-3 text-sm">
      <div className="font-medium text-warning">
        approval: {approval.tool} ({RISK[approval.risk]})
      </div>
      <pre className="mt-1 max-w-full overflow-x-auto font-mono text-xs leading-5 text-fg-secondary whitespace-pre-wrap">
        {describe(approval.tool, approval.args).join('\n')}
      </pre>
      <div className="mt-3 flex gap-2">
        <Button
          size="sm"
          className="pointer-coarse:h-11 pointer-coarse:flex-1"
          disabled={busy}
          onClick={() => decide('allow')}
        >
          Allow
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="pointer-coarse:h-11 pointer-coarse:flex-1"
          disabled={busy}
          onClick={() => decide('deny')}
        >
          Deny
        </Button>
      </div>
    </div>
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
                open session {p.session.slice(0, 8)}
              </Link>
              <ApprovalCard approval={p} />
            </div>
          ))}
        </div>
      </SheetContent>
    </Sheet>
  )
}
