// 210 §7.3: the approval card's share layout — exact public URL, source, access, expiry, the warning, the owner-only
// note; no "always allow" (a human-only tool asks every time, §7.2). `ApprovalCard` keeps the frame and the answer.

import { TriangleAlert } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { describeSharePublish } from '@/lib/describe'

export function SharePublishApproval({ args, busy, decide }: { args: unknown; busy: boolean; decide: (d: 'allow' | 'deny') => void }) {
  const ask = describeSharePublish(args, Date.now())
  if (!ask) return null
  const where = [ask.label && `label ${ask.label}`, ask.name && `name ${ask.name}`, ask.path && `path ${ask.path}`].filter(Boolean).join(' · ')
  return (
    <div data-share-approval="" className="my-2 rounded-xl border-2 border-danger bg-danger-bg px-4 py-3 text-sm">
      <div className="flex items-center gap-2 font-medium text-danger">
        <TriangleAlert className="size-4 shrink-0" />
        Publish to the public internet
        <span className="ml-auto text-xs font-normal">destructive</span>
      </div>
      <div className="mt-2 font-mono text-sm wrap-anywhere">
        {ask.url ?? <span className="text-fg-secondary">The edge assigns the address on publish{where ? ` (${where})` : ''}.</span>}
      </div>
      <dl className="mt-2 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-0.5 text-xs">
        <dt className="text-fg-secondary">Source</dt>
        <dd className="font-mono wrap-anywhere">{ask.source}</dd>
        <dt className="text-fg-secondary">Access</dt>
        <dd>{ask.access}</dd>
        <dt className="text-fg-secondary">Expires</dt>
        <dd>{ask.expiry}</dd>
      </dl>
      <p className="mt-3 text-xs">
        <strong>This makes it reachable by anyone on the internet</strong> who has the link. The app sees visitors as coming from
        this computer: do not share anything that trusts "localhost".
      </p>
      <p className="mt-1 text-xs text-fg-secondary">Only you can answer this, in person: not the agent, your double, an app or another chat user.</p>
      <div className="mt-3 flex justify-between gap-2">
        <Button size="sm" variant="outline" className="pointer-coarse:h-11 pointer-coarse:flex-1" disabled={busy} onClick={() => decide('deny')}>
          Deny
        </Button>
        <Button size="sm" variant="destructive" className="pointer-coarse:h-11 pointer-coarse:flex-1" disabled={busy} onClick={() => decide('allow')}>
          Publish for {ask.expiryLabel}
        </Button>
      </div>
    </div>
  )
}
