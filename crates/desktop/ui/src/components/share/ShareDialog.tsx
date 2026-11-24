import { useQuerySupport } from '@/rpc/capabilities'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, Share2 } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Action } from '@/components/editor/DocumentPage'
import { type Mesh, inMesh, machineName } from '@/components/notes/machine'
import { ago } from '@/components/sidebar/AgentTree'
import { useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import type { EngineProvider } from '@/lib/doc/provider'
import { useApi } from '@/lib/rpc'
import type { LinkCreated } from "@/rpc/bindings/method.link.output"
import type { LinkVia } from "@/rpc/bindings/method.link.output"
import type { Members } from "@/rpc/bindings/method.agent.output"
import type { Role } from "@/rpc/bindings/method.agent.output"

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

/** `doc_link_create` refusal (link crate `MESH_LINK_UNAVAILABLE`): Link is off on the home and no hub answers. */
const UNAVAILABLE = 'mesh_link_unavailable'

/** 184 §12.2: the description and small line in a mesh (null outside one: today's copy). Before a link exists `via` is unknown: a homed note reads as `home`. */
export function shareCopy(mesh: Mesh, home: string | null, via: LinkVia | undefined, role: Role, unavailable: boolean): { text: string; small: string } | null {
  if (!inMesh(mesh)) return null
  const name = home ? machineName(mesh, home) : ''
  const can = role === 'viewer' ? 'can view' : 'can edit'
  if (!home) return { text: `Anyone with this link ${can} the document. It is served by your mesh.`, small: 'Older links keep working.' }
  if (unavailable) return { text: `Link sharing is off on ${name}, and no hub is reachable to make a mesh link.`, small: `Turn on Relay in Settings › Network › Devices on ${name}, or retry when the hub is back.` }
  if (via === 'mesh') return { text: `Anyone with this link ${can} the note. It is served by your mesh, which passes it to ${name}.`, small: `Link sharing is off on ${name}, so this is a mesh link. Older links keep working.` }
  return { text: `Anyone with this link ${can} the note. It is served by ${name}; if ${name} is offline, by your mesh.`, small: 'Older links keep working.' }
}

/** The "Share…" action of a document page (59 §8.2): opens `ShareDialog` for the live document. */
export function ShareAction({ provider }: { provider: EngineProvider }) {
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
export function ShareDialog({ provider, open, onOpenChange }: { provider: EngineProvider; open: boolean; onOpenChange: (o: boolean) => void }) {
  const canQuery1 = useQuerySupport(["doc_members"])
  const api = useApi()
  const qc = useQueryClient()
  const doc = provider.id
  const key = ['doc', 'members', doc]
  const members = useQuery({ queryKey: key, queryFn: () => api.doc_members(doc), enabled: canQuery1 && (open) })
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
  /** The member whose Remove confirm is open. */
  const [removing, setRemoving] = useState<string | null>(null)
  const fail = (e: Error) => toast.error(e.message)
  const create = useMutation({
    mutationFn: () => api.doc_link_create(doc, role, preset.trim() || null, expiry === null ? null : Date.now() + expiry * DAY_MS),
    onSuccess: (r) => {
      setCreated(r)
      setPreset('')
    },
    onError: (e) => !e.message.includes(UNAVAILABLE) && fail(e),
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
  const mesh = useMesh().data
  const unavailable = !!create.error?.message.includes(UNAVAILABLE) && !created
  const text = shareCopy(mesh, provider.meta?.home ?? null, created?.via, created ? created.link.role : role, unavailable)
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{text ? `Share "${provider.meta?.title ?? ''}"` : 'Share'}</DialogTitle>
          <DialogDescription>
            {text?.text ?? 'Anyone with a link connects to this Mac through the relay and edits live; a viewer link only shows the document.'}
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
          {text && (
            <p className="flex items-center gap-2 text-xs text-fg-tertiary">
              {text.small}
              {unavailable && (
                <Button size="sm" variant="outline" disabled={create.isPending} onClick={() => create.mutate()}>
                  Retry
                </Button>
              )}
            </p>
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
              <li key={m.pub} className="flex flex-wrap items-center gap-2 py-1" data-member={m.pub}>
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
                <Button variant="outline" size="sm" disabled={revokeMember.isPending} onClick={() => setRemoving(m.pub)}>
                  Remove
                </Button>
                {removing === m.pub && (
                  <RemoveMember
                    name={m.name}
                    link={links.find((l) => l.id === m.via) ?? null}
                    onCancel={() => setRemoving(null)}
                    onConfirm={(alsoLink) => {
                      setRemoving(null)
                      revokeMember.mutate(m.pub)
                      if (alsoLink && m.via) revokeLink.mutate(m.via)
                    }}
                  />
                )}
              </li>
            ))}
          </ul>
        </section>
      </DialogContent>
    </Dialog>
  )
}

/**
 * Z184-R3: Remove's confirm. The engine refuses the member's key on every link made before the
 * remove, but anyone else holding the link they came in on can still join: offer to revoke it too.
 */
export function RemoveMember({
  name,
  link,
  onConfirm,
  onCancel,
}: {
  name: string
  /** The live link the member joined through, if any. */
  link: { id: string; name: string | null } | null
  onConfirm(alsoLink: boolean): void
  onCancel(): void
}) {
  const [alsoLink, setAlsoLink] = useState(true)
  const label = link?.name ?? 'without a name'
  return (
    <div role="group" aria-label={`Remove ${name}`} className="flex basis-full flex-col gap-1.5 rounded-md border border-border-subtle p-2 text-xs">
      <p>
        {name} can no longer join through any link made so far.
        {link && ` Without revoking link «${label}», anyone else holding it can still join.`}
      </p>
      {link && (
        <label className="flex items-center gap-1.5">
          <input type="checkbox" checked={alsoLink} onChange={(e) => setAlsoLink(e.target.checked)} />
          Also revoke link «{label}»
        </label>
      )}
      <span className="flex gap-1.5">
        <Button variant="outline" size="sm" onClick={() => onConfirm(link !== null && alsoLink)}>
          Remove {name}
        </Button>
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
      </span>
    </div>
  )
}
