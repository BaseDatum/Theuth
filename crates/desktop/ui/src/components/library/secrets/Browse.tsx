import { useInfiniteQuery } from '@tanstack/react-query'
import { Copy, Send, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { notify } from '@/lib/notify'
import { queryKeys } from '@/lib/rpc'
import { useApi } from '@/lib/rpc'
import { placeholderOf, valueRefs } from './labels'
import { Reveal } from './Reveal'
import { SendDialog, type SendSource } from './SendDialog'
import type { ConnInfo, ExtEntry } from '@/rpc/bindings/method.secrets.output'

/** `2026-09-16T10:02:11Z` → the user's locale, day precision. */
function day(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleDateString(undefined, { dateStyle: 'medium' })
}

/** The group a row sits under: Bitwarden `org / collection / folder`; AWS the name's prefix path (`app/db/`). */
function groupOf(e: ExtEntry, backend: ConnInfo['backend']): string {
  if (backend === 'bw') return e.location || 'No folder'
  const i = e.name.lastIndexOf('/')
  return i < 0 ? '/' : e.name.slice(0, i + 1)
}

/** The key Copy reference and Send… use: a login's password, else the entry's first field; AWS the whole secret. */
function defaultKey(e: ExtEntry, backend: ConnInfo['backend']): string | null {
  if (backend === 'aws') return null
  return e.keys.includes('password') ? 'password' : (e.keys[0] ?? null)
}

/**
 * §9.1 browse: `list_external_secrets` (metadata only, the shape the tool returns), grouped by folder / prefix path, with
 * search and the Deleted filter (AWS pending deletion, the Bitwarden trash). Rows: Copy reference (the placeholder,
 * never a value), Send…, Delete (recoverable; Delete permanently only with `allow_permanent_delete`), Reveal per
 * value-bearing ref (D25, when `allow_reveal`); deleted rows: Restore. The user is the actor, so no approval card.
 */
export function Browse({ info }: { info: ConnInfo }) {
  const api = useApi()
  const [query, setQuery] = useState('')
  const [applied, setApplied] = useState('')
  const [deleted, setDeleted] = useState(false)
  const [open, setOpen] = useState<string | null>(null)
  const [sending, setSending] = useState<SendSource | null>(null)
  const list = useInfiniteQuery({
    queryKey: queryKeys.secretExternalList(info.conn, applied, deleted),
    queryFn: ({ pageParam }) => api.list_external_secrets(info.conn, { query: applied || null, cursor: pageParam, deleted }),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.next,
  })
  const remove = useAction((v: { ref: string; permanent: boolean }) => api.delete_external_secret(info.conn, v.ref, v.permanent ? 'permanent' : { recoverable: { days: null } }), {
    keys: [queryKeys.secretExternal],
    onSuccess: (out, v) => notify.success(out.recoverable_until ? `Deleted ${v.ref}; restorable until ${day(out.recoverable_until)}` : `Deleted ${v.ref} permanently`),
  })
  // Per row, so a name typed on one entry never carries to another.
  const [importNames, setImportNames] = useState<Record<string, string>>({})
  const setImportName = (row: string, name: string) => setImportNames((m) => ({ ...m, [row]: name }))
  const importRef = useAction((v: { row: string; ref: string; name: string }) => api.import_external_secret(info.conn, v.ref, v.name, null), {
    keys: [queryKeys.secrets],
    onSuccess: (s, v) => {
      setImportName(v.row, '')
      notify.success(`Imported as ${s.name} v${s.current}; {{secret:${s.name}}}`)
    },
  })
  const restore = useAction((ref: string) => api.restore_external_secret(info.conn, ref), {
    keys: [queryKeys.secretExternal],
    onSuccess: (_, ref) => notify.success(`Restored ${ref}`),
  })
  const entries = list.data?.pages.flatMap((p) => p.entries) ?? []
  const groups = Map.groupBy(entries, (e) => groupOf(e, info.backend))
  const copyRef = (e: ExtEntry) =>
    void navigator.clipboard.writeText(placeholderOf(e.ref, defaultKey(e, info.backend))).then(
      () => notify.success('Reference copied'),
      (err) => notify.error(errorText(err)),
    )
  return (
    <div className="mb-3 ml-4 flex flex-col gap-2 border-l border-border-subtle pl-4 text-[13px]">
      {sending && <SendDialog open initial={sending} onOpenChange={(o) => !o && setSending(null)} />}
      <div className="flex items-center gap-2">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && setApplied(query.trim())}
          onBlur={() => setApplied(query.trim())}
          placeholder={info.backend === 'aws' ? 'Search names' : 'Search names, folders, collections'}
          aria-label={`Search ${info.conn}`}
          className="h-8"
        />
        <label className="flex shrink-0 items-center gap-2 text-fg-secondary">
          Deleted
          <Switch aria-label={`Show deleted entries of ${info.conn}`} checked={deleted} onCheckedChange={setDeleted} />
        </label>
      </div>
      {list.isError && <div className="text-danger">{errorText(list.error)}</div>}
      {list.isSuccess && entries.length === 0 && <Empty>{deleted ? 'Nothing deleted.' : applied ? 'Nothing matches.' : 'No entries.'}</Empty>}
      {[...groups].map(([group, rows]) => (
        <div key={group}>
          <div className="pt-2 text-xs font-medium text-fg-tertiary">{group}</div>
          {rows.map((e) => (
            <div key={e.ref} className="border-b border-border-subtle py-1.5 last:border-0">
              <div className="flex items-center justify-between gap-3">
                <button type="button" className="min-w-0 text-left" onClick={() => setOpen(open === e.ref ? null : e.ref)}>
                  <div className="truncate">{e.name}</div>
                  <div className="truncate text-xs text-fg-tertiary">
                    {e.kind}
                    {e.keys.length > 0 && <> · {e.keys.join(', ')}</>}
                    {e.versions[0] && <> · {e.versions[0].stages[0] ?? 'rev'} {day(e.versions[0].changed ?? e.versions[0].id)}</>}
                    {deleted && e.recoverable_until && <> · restorable until {day(e.recoverable_until)}</>}
                  </div>
                </button>
                <div className="flex shrink-0 items-center gap-1">
                  {deleted ? (
                    <Button variant="outline" size="sm" disabled={restore.isPending} onClick={() => restore.mutate(e.ref)}>
                      Restore
                    </Button>
                  ) : (
                    <>
                      <Button variant="ghost" size="icon-sm" aria-label={`Copy reference to ${e.name}`} onClick={() => copyRef(e)}>
                        <Copy />
                      </Button>
                      {e.kind !== 'binary' && (
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={`Send ${e.name}`}
                          onClick={() => {
                            const key = defaultKey(e, info.backend)
                            const ref = key ? `${e.ref}#${key}` : e.ref
                            setSending({ kind: 'text', source: ref, label: ref })
                          }}
                        >
                          <Send />
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`Delete ${e.name}`}
                        disabled={remove.isPending}
                        onClick={() =>
                          confirm(info.backend === 'aws' ? `Delete ${e.name}? AWS keeps it for ${info.settings.delete_recovery_days} days; Restore undoes it.` : `Move ${e.name} to the Bitwarden trash?`) &&
                          remove.mutate({ ref: e.ref, permanent: false })
                        }
                      >
                        <Trash2 />
                      </Button>
                      {info.settings.allow_permanent_delete && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-danger"
                          disabled={remove.isPending}
                          onClick={() => confirm(`Delete ${e.name} permanently? This cannot be undone.`) && remove.mutate({ ref: e.ref, permanent: true })}
                        >
                          Delete permanently
                        </Button>
                      )}
                    </>
                  )}
                </div>
              </div>
              {open === e.ref && !deleted && (
                <div className="mt-1 flex flex-col gap-1 pl-2">
                  <div className="font-mono text-xs text-fg-tertiary">{e.ref}</div>
                  <form
                    className="flex items-center gap-2"
                    onSubmit={(ev) => {
                      ev.preventDefault()
                      const key = defaultKey(e, info.backend)
                      const name = (importNames[e.ref] ?? '').trim()
                      if (name) importRef.mutate({ row: e.ref, ref: key ? `${e.ref}#${key}` : e.ref, name })
                    }}
                  >
                    <Input value={importNames[e.ref] ?? ''} onChange={(ev) => setImportName(e.ref, ev.target.value)} placeholder="Local name" aria-label={`Local name for ${e.name}`} className="h-7 w-48 font-mono text-xs" spellCheck={false} />
                    <Button type="submit" variant="outline" size="xs" disabled={!(importNames[e.ref] ?? '').trim() || importRef.isPending}>
                      Import to vault
                    </Button>
                    <span className="text-xs text-fg-tertiary">a snapshot; it does not follow rotations</span>
                  </form>
                  {info.settings.allow_reveal &&
                    valueRefs(e, info.backend).map((v) => (
                      <div key={v.ref} className="flex items-center justify-between gap-2">
                        <span className="font-mono text-xs">{v.label}</span>
                        <Reveal conn={info.conn} refText={v.ref} label={v.label} allowed={info.settings.allow_reveal} />
                      </div>
                    ))}
                </div>
              )}
            </div>
          ))}
        </div>
      ))}
      {list.hasNextPage && (
        <Button variant="ghost" size="sm" disabled={list.isFetchingNextPage} onClick={() => void list.fetchNextPage()}>
          More
        </Button>
      )}
    </div>
  )
}
