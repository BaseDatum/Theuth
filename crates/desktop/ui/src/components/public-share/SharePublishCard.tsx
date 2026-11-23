// 210 §7.4 item 3: the transcript card of a `share_publish` call — the public URL (copy, open), source, state, time
// left, visitors and last visit (this engine's counters), Stop. A refusal for unaccepted terms carries the first-run
// terms step (§7.6 step 3): accept here, then ask the agent to publish again.

import { Copy, ExternalLink, Globe } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { PasswordOnce, useSharePassword } from '@/components/public-share/PasswordOnce'
import { stopShare } from '@/components/public-share/actions'
import { type Share, shareApi } from '@/components/public-share/wire'
import { Button } from '@/components/ui/button'
import { openExternal } from '@/lib/platform'
import type { Entry } from '@/store/sessions'
import { type TermsAsk, edgePendingLabel, machineLabel, parseTermsError, sourceLabel, stateLabel, timeLeft, usePublicShares } from '@/store/public-shares'

type Tool = Extract<Entry, { kind: 'tool' }>

/** `Published https://… (live)`: the tool's text output (`crates/share/src/tools.rs`). */
const PUBLISHED = /^Published (\S+) \((\w+)\)/

/** The share this call created: `created_by` names the session and call; else the URL of the receipt. */
export function shareOfCall(shares: Record<string, Share>, session: string, callId: string, url: string | null): Share | null {
  const all = Object.values(shares)
  return (
    all.find((s) => typeof s.created_by === 'object' && s.created_by.session === session && s.created_by.call_id === callId) ??
    (url ? all.find((s) => s.url === url) : undefined) ??
    null
  )
}

export function SharePublishCard({ tool, session }: { tool: Tool; session: string }) {
  const receipt = tool.result && !tool.result.isError ? PUBLISHED.exec(tool.result.output) : null
  const url = receipt?.[1] ?? null
  const share = usePublicShares((s) => shareOfCall(s.shares, session, tool.callId, url))
  const traffic = usePublicShares((s) => (share ? s.traffic[share.id] : undefined))
  const terms = tool.result?.isError ? parseTermsError(tool.result.output) : null
  const machines = usePublicShares((s) => s.machines)
  const now = useNow(share?.state === 'live' || share?.state === 'offline')
  const running = !tool.interrupted && (tool.pending || !tool.result)
  // The tool output never carries the password (the model must not see it): the owner gets it once by `share_password`.
  const [password, hidePassword] = useSharePassword(share && share.access === 'password' && share.state !== 'stopped' ? share.id : null)
  const edgePending = usePublicShares((s) => s.edgePending)

  if (terms) return <TermsCard terms={terms} />
  if (running) return <Frame title="Publishing to the public internet…" />
  if (tool.result?.isError || !url) return <Frame title="Not published">{tool.result && <p className="mt-1 text-xs text-destructive wrap-anywhere">{tool.result.output}</p>}</Frame>

  const active = share ? share.state !== 'stopped' : true
  const left = share ? timeLeft(share.expires_ms, now) : null
  return (
    <Frame title={active ? 'Public on the internet' : 'Public share ended'} live={active}>
      <div className="mt-1 flex min-w-0 items-center gap-1">
        <button type="button" onClick={() => openExternal(url)} className="min-w-0 truncate font-mono text-xs text-fg-accent hover:underline" title="Open">
          {url}
        </button>
        <Button variant="ghost" size="icon" className="size-6" aria-label="Copy link" onClick={() => void navigator.clipboard.writeText(url).then(() => toast.success('Link copied'))}>
          <Copy className="size-3.5" />
        </Button>
        <Button variant="ghost" size="icon" className="size-6" aria-label="Open" onClick={() => openExternal(url)}>
          <ExternalLink className="size-3.5" />
        </Button>
      </div>
      {share && (
        <dl className="mt-1 grid grid-cols-[max-content_1fr] gap-x-3 text-xs text-fg-secondary">
          <dt>Source</dt>
          <dd className="truncate font-mono">
            {sourceLabel(share.source)}
            {machines && share.engine !== machines.self && <span className="font-sans"> on {machineLabel(share.engine, machines)}</span>}
          </dd>
          <dt>Access</dt>
          <dd>{share.access === 'password' ? 'Password' : 'Anyone with the link'}</dd>
          <dt>State</dt>
          <dd>{stateLabel(share)}</dd>
          {left !== null && active && (
            <>
              <dt>Time left</dt>
              <dd>{left}</dd>
            </>
          )}
          {traffic && (
            <>
              <dt>Visitors</dt>
              <dd>
                {traffic.visitors}
                {traffic.last_ms !== null && ` · last ${new Date(traffic.last_ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`}
              </dd>
            </>
          )}
        </dl>
      )}
      {password && <PasswordOnce password={password} onDone={hidePassword} />}
      {share && !active && edgePending !== null && <p className="mt-1 text-xs text-danger">{edgePendingLabel(edgePending)}</p>}
      {share && active && (
        <div className="mt-2 flex gap-2">
          {/* Never gated (§7.4). TODO(210-M2): Extend (re-asks) once `share_extend` exists. */}
          <Button size="sm" variant="destructive" onClick={() => void stopShare(share.id)}>
            Stop
          </Button>
        </div>
      )}
    </Frame>
  )
}

function Frame({ title, live, children }: { title: string; live?: boolean; children?: ReactNode }) {
  return (
    <div className={`my-2 rounded-xl border px-4 py-3 text-sm ${live ? 'border-danger bg-danger-bg' : 'border-border-subtle'}`}>
      <div className={`flex items-center gap-2 font-medium ${live ? 'text-danger' : ''}`}>
        <Globe className="size-4 shrink-0" />
        {title}
      </div>
      {children}
    </div>
  )
}

function TermsCard({ terms }: { terms: TermsAsk }) {
  const [ok, setOk] = useState(false)
  const [done, setDone] = useState(false)
  // Settings › Sharing › Terms offers the same step.
  useEffect(() => usePublicShares.getState().askTerms(terms), [terms.edge, terms.url, terms.rev])
  const accept = async () => {
    try {
      await shareApi().share_terms_accept(terms.edge, terms.rev)
      setDone(true)
      usePublicShares.getState().askTerms(null)
      toast.success('Terms accepted: ask again to publish')
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return (
    <Frame title="Accept the edge's terms to publish">
      <p className="mt-1 text-xs text-fg-secondary">
        The sharing edge needs its terms accepted once before anything goes public.{' '}
        <button type="button" className="text-fg-accent hover:underline" onClick={() => openExternal(terms.url)}>
          Read the terms (revision {terms.rev})
        </button>
      </p>
      {done ? (
        <p className="mt-2 text-xs">Accepted. Ask the agent to publish again.</p>
      ) : (
        <div className="mt-2 flex items-center gap-3">
          <label className="flex items-center gap-2 text-xs">
            <input type="checkbox" checked={ok} onChange={(e) => setOk(e.target.checked)} />I accept
          </label>
          <Button size="sm" disabled={!ok} onClick={() => void accept()}>
            Accept
          </Button>
        </div>
      )}
    </Frame>
  )
}

/** Re-render every 30 s while `on`, for the countdown. */
function useNow(on: boolean): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!on) return
    const t = window.setInterval(() => setNow(Date.now()), 30_000)
    return () => window.clearInterval(t)
  }, [on])
  return now
}
