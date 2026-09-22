import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, Share2 } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Action } from '@/components/editor/DocumentPage'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import type { HarnessProvider } from '@/lib/doc/provider'
import { useApi } from '@/lib/rpc'
import type { LinkCreated } from '@/rpc/bindings/LinkCreated'
import type { Members } from '@/rpc/bindings/Members'
import type { Role } from '@/rpc/bindings/Role'

const DAY_MS = 86_400_000
/** Expiry choices (59 §8.2): days, or never. */
const EXPIRY: { label: string; days: number | null }[] = [
  { label: '1 day', days: 1 },
  { label: '7 days', days: 7 },
  { label: '30 days', days: 30 },
  { label: 'Never', days: null },
]

const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast.message('Copied'))
const at = (ms: bigint | number) => ago(new Date(Number(ms)).toISOString())

/** The "Share…" action of a document page (59 §8.2): opens `ShareDialog` for the live document. */
export function ShareAction({ provider }: { provider: HarnessProvider }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Action label="Share…" icon={Share2} onClick={() => setOpen(true)} />
      {open && <ShareDialog provider={provider} open={open} onOpenChange={setOpen} />}
    </>
  )
}

/**
 * Links and members of one document (59 §8.2, §8.4): create a link (role, preset name, expiry) and
 * see its URL once with copy + QR; revoke links; rename, re-role and remove members. The lists are
 * live through the subscription's `Members` events.
 */
export function ShareDialog({ provider, open, onOpenChange }: { provider: HarnessProvider; open: boolean; onOpenChange: (o: boolean) => void }) {
  const api = useApi()
  const qc = useQueryClient()
  const doc = provider.id
  const key = ['doc', 'members', doc]
  const members = useQuery({ queryKey: key, queryFn: () => api.doc_members(doc), enabled: open })
  useEffect(
    () =>
      provider.onEvent((e) => {
        if (e.type === 'Members') qc.setQueryData<Members>(key, { members: e.members, links: e.links })
      }),
    [provider, qc, doc],
  )
  const [role, setRole] = useState<Role>('editor')
  const [preset, setPreset] = useState('')
  const [expiry, setExpiry] = useState<number | null>(7)
  const [created, setCreated] = useState<LinkCreated | null>(null)
  const fail = (e: Error) => toast.error(e.message)
  const create = useMutation({
    mutationFn: () => api.doc_link_create(doc, role, preset.trim() || null, expiry === null ? null : Date.now() + expiry * DAY_MS),
    onSuccess: (r) => {
      setCreated(r)
      setPreset('')
    },
    onError: fail,
  })
  const revokeLink = useMutation({ mutationFn: (link: string) => api.doc_link_revoke(doc, link), onError: fail })
  const setMember = useMutation({
    mutationFn: (m: { member: string; role?: Role; name?: string }) => api.doc_member_set(doc, m.member, m.role ?? null, m.name ?? null),
    onError: fail,
  })
  const revokeMember = useMutation({
    mutationFn: (member: string) => api.doc_member_revoke(doc, member),
    onSuccess: () => toast('Access removed'),
    onError: fail,
  })
  const links = members.data?.links.filter((l) => l.revoked_ms === null) ?? []
  const nameOf = (id: string | null) => members.data?.links.find((l) => l.id === id)?.name
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Share</DialogTitle>
          <DialogDescription>
            Anyone with a link connects to this Mac through the relay and edits live; a viewer link only shows the document.
          </DialogDescription>
        </DialogHeader>

        <section className="space-y-2" aria-label="New link">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <select className="h-8 rounded-md border border-border-subtle bg-transparent px-2" value={role} onChange={(e) => setRole(e.target.value as Role)} aria-label="Role">
              <option value="editor">Can edit</option>
              <option value="viewer">Can view</option>
            </select>
            <select
              className="h-8 rounded-md border border-border-subtle bg-transparent px-2"
              value={expiry ?? 'never'}
              onChange={(e) => setExpiry(e.target.value === 'never' ? null : Number(e.target.value))}
              aria-label="Expires"
            >
              {EXPIRY.map((x) => (
                <option key={x.label} value={x.days ?? 'never'}>
                  {x.days === null ? 'Never expires' : `Expires in ${x.label}`}
                </option>
              ))}
            </select>
            <Input className="h-8 w-40" placeholder="Their name (optional)" value={preset} onChange={(e) => setPreset(e.target.value)} aria-label="Preset name" />
            <Button size="sm" disabled={create.isPending} onClick={() => create.mutate()}>
              Create link
            </Button>
          </div>
          {created && (
            <div className="flex gap-3 rounded-md border border-border-subtle p-3" data-testid="share-url">
              <QRCodeSVG value={created.url} size={112} marginSize={1} />
              <div className="min-w-0 flex-1 space-y-2 text-xs">
                <p className="text-fg-tertiary">Shown once — copy it now. {created.link.role === 'viewer' ? 'Viewers' : 'Editors'} who open it join as {created.link.name ?? 'the name they choose'}.</p>
                <code className="block break-all rounded bg-bg-secondary p-2 select-all">{created.url}</code>
                <Button size="sm" variant="outline" onClick={() => void copy(created.url)}>
                  <Copy /> Copy link
                </Button>
              </div>
            </div>
          )}
        </section>

        <section aria-label="Links">
          <h3 className="mb-1 text-xs font-medium text-fg-tertiary">Links</h3>
          {links.length === 0 && <p className="text-sm text-fg-tertiary">No active links.</p>}
          <ul className="text-sm">
            {links.map((l) => (
              <li key={l.id} className="flex items-center gap-2 py-1">
                <span className="flex-1 truncate">
                  {l.role === 'viewer' ? 'View' : 'Edit'}
                  {l.name ? ` · ${l.name}` : ''}
                </span>
                <span className="text-xs text-fg-tertiary">
                  created {at(l.created_ms)} ago
                  {l.expires_ms === null ? '' : ` · expires ${new Date(Number(l.expires_ms)).toLocaleDateString()}`}
                </span>
                <Button variant="outline" size="sm" disabled={revokeLink.isPending} onClick={() => revokeLink.mutate(l.id)}>
                  Revoke
                </Button>
              </li>
            ))}
          </ul>
        </section>

        <section aria-label="Members">
          <h3 className="mb-1 text-xs font-medium text-fg-tertiary">Members</h3>
          {members.data?.members.length === 0 && <p className="text-sm text-fg-tertiary">Nobody has joined yet.</p>}
          <ul className="text-sm">
            {members.data?.members.map((m) => (
              <li key={m.pub} className="flex items-center gap-2 py-1" data-member={m.pub}>
                <span className="size-2.5 shrink-0 rounded-full" style={{ background: m.color }} aria-hidden />
                <span className="min-w-0 flex-1 truncate">
                  {m.name}
                  <span className="text-xs text-fg-tertiary">
                    {nameOf(m.via) ? ` · via link ${nameOf(m.via)}` : m.via ? ' · via link' : ''} · seen {at(m.last_seen_ms)}
                  </span>
                </span>
                <select
                  className="h-7 rounded-md border border-border-subtle bg-transparent px-1 text-xs"
                  value={m.role}
                  aria-label={`Role of ${m.name}`}
                  onChange={(e) => setMember.mutate({ member: m.pub, role: e.target.value as Role })}
                >
                  <option value="editor">editor</option>
                  <option value="viewer">viewer</option>
                </select>
                <Button variant="outline" size="sm" disabled={revokeMember.isPending} onClick={() => revokeMember.mutate(m.pub)}>
                  Remove
                </Button>
              </li>
            ))}
          </ul>
        </section>
      </DialogContent>
    </Dialog>
  )
}
