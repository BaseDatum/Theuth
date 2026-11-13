import { Link } from '@tanstack/react-router'
import { UserPlus } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import type { AgentSummary } from "@/rpc/bindings/method.roster.output"
import { AgentAvatar } from './identity'
import { useAgentActions } from './queries'

/**
 * Paperclip's pending `hire_agent` approvals as a strip above the roster: the only place a *proposed* agent appears
 * (a hire an agent proposed, 67 §6.5, awaiting your approval). Approve (`agent_approve`) makes it active; Decline
 * (`agent_remove` with a note for the hire task) deletes the file. The row opens the detail page.
 */
export function HiringInbox({ agents }: { agents: AgentSummary[] }) {
  const { approve, reject } = useAgentActions()
  const [declining, setDeclining] = useState<AgentSummary | null>(null)
  const [note, setNote] = useState('')
  if (agents.length === 0) return null
  const busy = approve.isPending || reject.isPending
  return (
    <section aria-label="Hiring" className="shrink-0 border-b border-border-subtle bg-layer-1">
      <h3 className="flex items-center gap-2 px-3 pt-2 text-xs font-semibold uppercase tracking-wide text-fg-secondary">
        <UserPlus className="size-3.5 text-primary" /> Hiring · {agents.length} proposed {agents.length === 1 ? 'hire awaits' : 'hires await'} your approval
      </h3>
      <ul className="divide-y divide-border-subtle">
        {agents.map((a) => (
          <li key={a.file.name} className="flex items-center gap-3 px-3 py-2 text-[13px]">
            <AgentAvatar name={a.file.name} title={a.file.title} size={28} />
            <Link to="/agents/$name/$tab" params={{ name: a.file.name, tab: 'overview' }} className="min-w-0 flex-1 hover:underline">
              <b>{a.file.title}</b> <span className="font-mono text-fg-tertiary">{a.file.name}</span>
              <span className="block truncate text-fg-secondary">
                proposed by {a.file.proposed_by ?? '—'} · reports to {a.file.reports_to}
                {a.file.budget.type === 'monthly' && ` · ${a.file.budget.amount} ${a.file.budget.currency}/month`}
              </span>
            </Link>
            <Button size="sm" disabled={busy} onClick={() => approve.mutate(a.file.name)}>
              Approve
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => {
                setNote('')
                setDeclining(a)
              }}
            >
              Decline
            </Button>
          </li>
        ))}
      </ul>
      <Dialog open={declining !== null} onOpenChange={(o) => !o && setDeclining(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Decline {declining?.file.title}?</DialogTitle>
            <DialogDescription>
              The proposed file is deleted and the hire task is closed with your note, so {declining?.file.proposed_by ?? 'the proposer'} learns why.
            </DialogDescription>
          </DialogHeader>
          <Label htmlFor="decline-note" className="text-[13px]">
            Note to the proposer (optional)
          </Label>
          <Textarea id="decline-note" rows={3} value={note} onChange={(e) => setNote(e.target.value)} placeholder="e.g. we already have ops covered; fold this into sre" />
          <DialogFooter>
            <Button variant="ghost" onClick={() => setDeclining(null)}>
              Keep proposal
            </Button>
            <Button
              variant="destructive"
              disabled={reject.isPending}
              onClick={() => {
                if (declining) reject.mutate({ name: declining.file.name, note }, { onSuccess: () => setDeclining(null) })
              }}
            >
              Decline
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}
