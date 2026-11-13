import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Download, File, Folder } from 'lucide-react'
import { useMemo, useState } from 'react'
import { toast } from 'sonner'
import { saveAs } from '@/components/chat/ArtifactCard'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { fmtBytes, isLocal } from '@/lib/files'
import { privateOwner } from '@/lib/private'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useEngines } from '@/link/engines'
import type { ArchiveEntry } from "@/rpc/bindings/method.artifact.output"
import type { Artifact } from "@/rpc/bindings/method.artifact.output"

const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))

/** Cap the listing sidecar applies (`artifact::archive::MAX_ENTRIES`). */
const CAP = 5000

export type Node = { name: string; path: string; dir: boolean; size: number; mtime: number | null; children: Map<string, Node> }

/**
 * Member paths → a folder tree. Paths are display data only (design 28): split on `/`, never resolved, never linked.
 * A folder's size is the sum of what it holds.
 */
export function build(entries: ArchiveEntry[]): Node {
  const root: Node = { name: '', path: '', dir: true, size: 0, mtime: null, children: new Map() }
  for (const e of entries) {
    const parts = e.path.split('/').filter(Boolean)
    let node = root
    parts.forEach((p, i) => {
      const last = i === parts.length - 1
      let child = node.children.get(p)
      if (!child) {
        child = { name: p, path: parts.slice(0, i + 1).join('/'), dir: !last || e.is_dir, size: 0, mtime: null, children: new Map() }
        node.children.set(p, child)
      }
      if (last) {
        child.mtime = e.mtime
        if (!e.is_dir) child.size = e.size
      }
      node = child
    })
  }
  const sum = (n: Node): number => {
    if (n.dir) n.size = [...n.children.values()].reduce((acc, c) => acc + sum(c), 0)
    return n.size
  }
  sum(root)
  return root
}

const fmtDate = (t: number | null) => (t ? new Date(t * 1000).toLocaleDateString() : '')

function Rows({ node, depth, closed, toggle }: { node: Node; depth: number; closed: Set<string>; toggle: (p: string) => void }) {
  // Folders first, then files, each alphabetically.
  const children = [...node.children.values()].sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name))
  return (
    <>
      {children.map((n) => {
        const open = n.dir && !closed.has(n.path)
        const Glyph = n.dir ? Folder : File
        const Chevron = open ? ChevronDown : ChevronRight
        return (
          <li key={n.path}>
            <div
              role={n.dir ? 'button' : undefined}
              tabIndex={n.dir ? 0 : undefined}
              onClick={n.dir ? () => toggle(n.path) : undefined}
              onKeyDown={n.dir ? (e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), toggle(n.path)) : undefined}
              className={cn(
                'flex h-8 items-center gap-1.5 rounded-md px-2 text-xs pointer-coarse:h-11 hover:bg-layer-2',
                n.dir && 'cursor-pointer select-none',
              )}
              style={{ paddingLeft: `${8 + depth * 16}px` }}
            >
              {n.dir ? <Chevron className="size-3.5 shrink-0 text-fg-tertiary" /> : <span className="size-3.5 shrink-0" />}
              <Glyph className="size-3.5 shrink-0 text-fg-tertiary" />
              <span className="min-w-0 flex-1 truncate font-mono" title={n.path}>
                {n.name}
                {n.dir && '/'}
              </span>
              <span className="w-16 shrink-0 text-right tabular-nums text-fg-secondary">{n.dir && n.size === 0 ? '' : fmtBytes(n.size)}</span>
              <span className="hidden w-24 shrink-0 text-right tabular-nums text-fg-tertiary sm:inline">{fmtDate(n.mtime)}</span>
            </div>
            {open && n.children.size > 0 && (
              <ul>
                <Rows node={n} depth={depth + 1} closed={closed} toggle={toggle} />
              </ul>
            )}
          </li>
        )
      })}
    </>
  )
}

/** An `archive` artifact (35 §3.2): totals, a filter, the member tree from the listing sidecar, and the download. */
export function ArchiveView({ a }: { a: Artifact }) {
  const canQuery1 = useQuerySupport(["get_artifact_listing"])
  const api = useApi()
  const active = useEngines((s) => s.active)
  const listing = useQuery({ enabled: canQuery1, queryKey: ['artifact-listing', a.call_id], queryFn: () => api.get_artifact_listing(a.call_id, privateOwner(a.session)), retry: false })
  const [q, setQ] = useState('')
  const [closed, setClosed] = useState<Set<string>>(new Set())
  const toggle = (p: string) =>
    setClosed((s) => {
      const next = new Set(s)
      if (!next.delete(p)) next.add(p)
      return next
    })
  const entries = listing.data?.entries
  const tree = useMemo(() => {
    if (!entries) return null
    const needle = q.trim().toLowerCase()
    return build(needle ? entries.filter((e) => e.path.toLowerCase().includes(needle)) : entries)
  }, [entries, q])
  const download = (
    <Button variant="outline" size="sm" onClick={() => saveAs(api, a, isLocal(active)).catch(fail)}>
      <Download />
      {isLocal(active) ? 'Save as…' : 'Download'}
    </Button>
  )
  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border-subtle px-4 py-3">
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium">{a.name}</div>
          <div className="text-xs text-fg-secondary">
            {listing.data
              ? `${listing.data.total_entries} ${listing.data.total_entries === 1 ? 'entry' : 'entries'} · ${fmtBytes(listing.data.total_size)} unpacked · ${fmtBytes(a.bytes)} archive`
              : `${a.mime} · ${fmtBytes(a.bytes)}`}
            {listing.data?.truncated && <span className="ml-2 text-warning">listing truncated at {CAP.toLocaleString()}</span>}
          </div>
        </div>
        {download}
      </div>
      {listing.isError ? (
        <div className="p-6 text-sm text-fg-secondary">No listing: the archive could not be read when it was shown. It can still be downloaded.</div>
      ) : (
        <>
          <div className="px-4 py-2">
            <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter paths…" aria-label="Filter paths" className="h-8 pointer-coarse:h-11" />
          </div>
          <div className="min-h-0 flex-1 overflow-auto px-2 pb-4">
            {tree ? (
              tree.children.size > 0 ? (
                <ul>
                  <Rows node={tree} depth={0} closed={closed} toggle={toggle} />
                </ul>
              ) : (
                <div className="px-2 py-6 text-center text-xs text-fg-tertiary">{q ? 'no matching paths' : 'empty archive'}</div>
              )
            ) : (
              <div className="m-2 h-40 animate-pulse rounded-xl bg-layer-2" />
            )}
          </div>
        </>
      )}
    </div>
  )
}
