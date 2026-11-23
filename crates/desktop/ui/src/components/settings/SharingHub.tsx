// 213 §2: Settings › Sharing's new tabs — Overview (counts and last changes from `sharing_list`), Chats and agents
// (the exports and the contacts granted each) and Notes and notebooks (documents with members or links). Revoke and
// stop are never behind a confirmation (210); a kind the engine reports in `partial` shows one muted line.

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { MessageSquare, X } from 'lucide-react'
import { toast } from 'sonner'
import { ExportChip, useChatMachine, who } from '@/components/settings/Contacts'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { type SharingKind, type SharingPartial, type SharingRow, ended, useSharingList } from '@/components/settings/sharing-wire'
import { agoText } from '@/components/sources/origin'
import { Button } from '@/components/ui/button'
import { names, useContacts, useExports } from '@/lib/contacts'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'

/** §2 rule: a kind whose plugin is off reads as one line, not an error. */
const CONTACTS_OFF = 'Contacts are off — turn on agent messaging in Network'

const KIND_LABEL: Record<SharingKind, string> = {
  document_member: 'Note member',
  document_link: 'Note link',
  public_share: 'Public share',
  export: 'Chat or agent',
  contact: 'Contact',
}

/** The muted line for a kind the engine could not list. */
export function partialLine(p: SharingPartial): string {
  if (p.kind === 'export' || p.kind === 'contact') return CONTACTS_OFF
  if (p.kind === 'public_share') return `Public shares are unavailable here (${p.reason})`
  return `Shared notes are unavailable here (${p.reason})`
}

function Partial({ partial }: { partial: readonly SharingPartial[] }) {
  const lines = [...new Set(partial.map(partialLine))]
  return lines.map((l) => (
    <p key={l} role="status" className="py-1 text-[13px] text-fg-tertiary">
      {l}
    </p>
  ))
}

const toTab = (tab: string) => ({ to: '/settings/$tab', params: { tab } }) as const

/** Overview's groups: what each counts and the tab that manages it. */
const GROUPS: { tab: string; label: string; kinds: readonly SharingKind[]; unit: (n: number) => string }[] = [
  { tab: 'shared-notes', label: 'Notes and notebooks', kinds: ['document_member', 'document_link'], unit: (n) => `${n} shared document${n === 1 ? '' : 's'}` },
  { tab: 'contacts', label: 'Contacts', kinds: ['contact'], unit: (n) => `${n} contact${n === 1 ? '' : 's'}` },
  { tab: 'shared-chats', label: 'Chats and agents', kinds: ['export'], unit: (n) => `${n} shared chat${n === 1 ? '' : 's'} or agent${n === 1 ? '' : 's'}` },
  { tab: 'sharing', label: 'Public shares', kinds: ['public_share'], unit: (n) => `${n} live` },
]

/** A row's latest known moment: its last access, else its creation. */
const lastMs = (r: SharingRow): number => Number(r.last_access_ms ?? r.created_ms ?? 0)

/** Overview's count of a group: documents (not rows) for notes, live rows otherwise. */
export function groupCount(rows: readonly SharingRow[], kinds: readonly SharingKind[]): number {
  const live = rows.filter((r) => kinds.includes(r.kind) && !ended(r.state))
  return kinds.includes('document_member') ? new Set(live.map((r) => r.parent ?? r.id)).size : live.length
}

/** 213 §2 tab 1: counts per kind, each linking to its tab, the last changes, and the way to Network's switches. */
export function SharingOverviewTab() {
  const navigate = useNavigate()
  const list = useSharingList(null)
  const rows = list.data?.rows ?? []
  const recent = rows.filter((r) => lastMs(r) > 0).sort((a, b) => lastMs(b) - lastMs(a)).slice(0, 8)
  const now = Date.now()
  return (
    <>
      <Section title="Shared">
        {list.isPending && <Empty>Loading…</Empty>}
        {list.isError && <Empty>{errorText(list.error)}</Empty>}
        {list.data && <Partial partial={list.data.partial} />}
        {list.data &&
          GROUPS.map((g) => (
            <Row key={g.tab} label={g.label} description={g.unit(groupCount(rows, g.kinds))}>
              <Button variant="outline" size="sm" onClick={() => void navigate(toTab(g.tab))}>
                Manage
              </Button>
            </Row>
          ))}
      </Section>
      {list.data && (
        <Section title="Last changes">
          {recent.length === 0 ? (
            <Empty>Nothing shared yet.</Empty>
          ) : (
            recent.map((r) => (
              <Row
                key={`${r.kind}|${r.id}`}
                label={<span className="wrap-anywhere">{r.title}</span>}
                description={[KIND_LABEL[r.kind], r.audience, r.state, r.last_access_ms != null ? `last used ${agoText(Number(r.last_access_ms), now)}` : r.created_ms != null && `added ${agoText(Number(r.created_ms), now)}`].filter(Boolean).join(' · ')}
              />
            ))
          )}
        </Section>
      )}
      <Section title="Network">
        <Row label="Turn features on or off" description="Machines, devices, agent messaging and the relay live in Settings › Network.">
          <Button variant="outline" size="sm" onClick={() => void navigate(toTab('machine'))}>
            Open Network
          </Button>
        </Row>
      </Section>
    </>
  )
}

/** 213 §2 tab 3: every export with the contacts granted it; Stop sharing with one, Remove the export, Open its chat.
 * Both go through `sharing_revoke` (`export`, with the contact for one grant); the engine fires `A2aChanged`. */
export function SharedChatsTab() {
  const api = useApi()
  const navigate = useNavigate()
  const exports = useExports()
  const contacts = useContacts()
  const machineOf = useChatMachine()
  const revoke = useAction((v: { name: string; contact: string | null }) => api.sharing_revoke('export', v.name, v.contact), {
    keys: [queryKeys.contacts, queryKeys.exports, queryKeys.sharing],
    onSuccess: (_r, v) => v.contact === null && toast.message(`Stopped sharing ${v.name}`),
  })
  const busy = revoke.isPending
  return (
    <Section title="Chats and agents">
      <p className="text-[13px] text-fg-secondary">Each chat and agent your contacts may reach. Share more from a contact under Contacts or from a chat’s menu.</p>
      {(exports.isError || contacts.isError) && (
        <p role="status" className="py-1 text-[13px] text-fg-tertiary">
          {CONTACTS_OFF}
        </p>
      )}
      {exports.isPending && <Empty>Loading…</Empty>}
      {exports.data?.length === 0 && <Empty>Nothing is shared with contacts.</Empty>}
      {exports.data?.map((x) => {
        const granted = (contacts.data ?? []).filter((c) => c.exports.includes(x.name))
        const label = x.description || x.name
        const session = x.target.kind === 'chat' ? x.target.session : null
        return (
          <div key={x.name} data-export={x.name} className="flex flex-col gap-1.5 border-b border-border-subtle py-3 last:border-0">
            <div className="flex flex-wrap items-center gap-2">
              <div className="flex min-w-0 flex-1 basis-40">
                <ExportChip name={x.name} exp={x} on={machineOf(x)} />
              </div>
              {session && (
                <Button variant="ghost" size="sm" onClick={() => void navigate({ to: '/c/$session', params: { session } })}>
                  <MessageSquare /> Open
                </Button>
              )}
              <Button variant="ghost" size="sm" className="text-danger" disabled={busy} onClick={() => revoke.mutate({ name: x.name, contact: null })}>
                Remove export
              </Button>
            </div>
            <p className="text-[13px] text-fg-secondary">{granted.length === 0 ? 'Shared with nobody.' : `Shared with ${names(granted)}`}</p>
            {granted.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {granted.map((c) => (
                  <span key={c.id || c.name} className="inline-flex items-center gap-0.5 rounded-full border border-border-subtle py-0.5 pr-0.5 pl-2 text-xs text-fg-secondary">
                    {c.name}
                    <Button variant="ghost" size="icon-sm" className="size-5" aria-label={`Stop sharing ${label} with ${c.name}`} disabled={busy} onClick={() => revoke.mutate({ name: x.name, contact: who(c) })}>
                      <X className="size-3" />
                    </Button>
                  </span>
                ))}
              </div>
            )}
          </div>
        )
      })}
    </Section>
  )
}

const day = (ms: number | bigint) => new Date(Number(ms)).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })

/** One document's members and links, grouped by `parent` (the doc id) in list order. */
export function byDocument(rows: readonly SharingRow[]): { doc: string; title: string; members: SharingRow[]; links: SharingRow[] }[] {
  const docs = new Map<string, { doc: string; title: string; members: SharingRow[]; links: SharingRow[] }>()
  for (const r of rows) {
    const doc = r.parent ?? r.id.split('|')[0]!
    const d = docs.get(doc) ?? { doc, title: r.title, members: [], links: [] }
    docs.set(doc, d)
    if (r.kind === 'document_member') d.members.push(r)
    else if (r.kind === 'document_link') d.links.push(r)
  }
  return [...docs.values()]
}

/** 213 §2 tab 4: documents with members or links; Remove member, Revoke link (and the members who joined through it), Open
 * (a notebook on its own page, from `notebook_list`; any other document as a note). */
export function SharedNotesTab() {
  const api = useApi()
  const nb = useNotebooksApi()
  const navigate = useNavigate()
  const list = useSharingList(['document_member', 'document_link'])
  const notebooks = useQuery({ enabled: useQuerySupport(['notebook_list']), queryKey: notebookKeys.list(false), queryFn: () => nb.notebook_list(false) })
  const notebook = new Set(notebooks.data?.map((n) => n.doc.id))
  const revoke = useAction((v: { kind: SharingKind; id: string; sub: string | null }) => api.sharing_revoke(v.kind, v.id, v.sub), { keys: [queryKeys.sharing] })
  const now = Date.now()
  const docs = byDocument(list.data?.rows ?? [])
  return (
    <Section title="Notes and notebooks">
      {list.isPending && <Empty>Loading…</Empty>}
      {list.isError && <Empty>{errorText(list.error)}</Empty>}
      {list.data && <Partial partial={list.data.partial} />}
      {list.data && docs.length === 0 && <Empty>No note is shared. Share one from its Share button.</Empty>}
      {docs.map((d) => (
        <div key={d.doc} data-doc={d.doc} className="border-b border-border-subtle py-3 last:border-0">
          <div className="flex items-center gap-2">
            <span className="min-w-0 flex-1 truncate text-sm font-medium">{d.title}</span>
            <Button variant="outline" size="sm" onClick={() => void navigate(notebook.has(d.doc) ? { to: '/notebooks/$id', params: { id: d.doc } } : { to: '/notes/$id', params: { id: d.doc } })}>
              Open document
            </Button>
          </div>
          <ul className="mt-1.5 text-sm" aria-label={`Members of ${d.title}`}>
            {d.members.length === 0 && <li className="py-1 text-[13px] text-fg-tertiary">Nobody has joined.</li>}
            {d.members.map((m) => (
              <li key={m.id} className="flex flex-wrap items-center gap-2 py-1">
                <span className="min-w-0 flex-1 truncate">
                  {m.audience}
                  <span className="text-xs text-fg-tertiary">
                    {[m.permission, m.last_access_ms != null && `seen ${agoText(Number(m.last_access_ms), now)}`].filter(Boolean).map((s) => ` · ${s}`)}
                  </span>
                </span>
                {m.revocable && (
                  <Button variant="outline" size="sm" disabled={revoke.isPending} onClick={() => revoke.mutate({ kind: 'document_member', id: m.id, sub: null })}>
                    Remove
                  </Button>
                )}
              </li>
            ))}
          </ul>
          {d.links.length > 0 && (
            <ul className="mt-1 text-sm" aria-label={`Links of ${d.title}`}>
              {d.links.map((l) => {
                const joined = Number(l.count ?? 0)
                const live = !ended(l.state)
                return (
                  <li key={l.id} className={`flex flex-wrap items-center gap-2 py-1${live ? '' : ' text-fg-tertiary'}`}>
                    <span className="min-w-0 flex-1 truncate">
                      Link{l.permission ? ` · ${l.permission}` : ''}
                      <span className="text-xs text-fg-tertiary">
                        {[
                          l.audience,
                          l.created_ms != null && `created ${day(l.created_ms)}`,
                          l.expires_ms != null && `expires ${day(l.expires_ms)}`,
                          l.state,
                          joined > 0 && `${joined} joined`,
                        ]
                          .filter(Boolean)
                          .map((s) => ` · ${s}`)}
                      </span>
                    </span>
                    {live && l.revocable && (
                      <Button variant="outline" size="sm" disabled={revoke.isPending} onClick={() => revoke.mutate({ kind: 'document_link', id: l.id, sub: null })}>
                        Revoke link
                      </Button>
                    )}
                    {/* A revoked or expired link still sweeps the members who came through it. */}
                    {l.revocable && joined > 0 && (
                      <Button variant="outline" size="sm" disabled={revoke.isPending} onClick={() => revoke.mutate({ kind: 'document_link', id: l.id, sub: 'members' })}>
                        {live ? 'Revoke and remove its members' : 'Remove its members'}
                      </Button>
                    )}
                  </li>
                )
              })}
            </ul>
          )}
        </div>
      ))}
    </Section>
  )
}
