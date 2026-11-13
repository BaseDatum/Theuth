import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { BookOpen, ExternalLink, FolderOpen, ListChecks, type LucideIcon, MessageSquare, Mic, StickyNote } from 'lucide-react'
import type { ReactNode } from 'react'
import { ArtifactView } from '@/components/artifacts/Viewer'
import { Excerpt } from '@/components/chat/surface/Excerpt'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { extOf } from '@/lib/files'
import { openExternal } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Pointer } from "@/rpc/bindings/method.roster.output"
import { parseCodeRef, pointerRoute, urlHost } from './model'
import { UnifiedDiff } from './UnifiedDiff'

const ICON: Partial<Record<Pointer['kind'], LucideIcon>> = { artifact: FolderOpen, note: StickyNote, knowledge: BookOpen, session: MessageSquare, meeting: Mic, task: ListChecks, url: ExternalLink }

/** 101 D10: the agent's pointers, one block each — inline evidence for `code` / `diff` / `artifact`, a linking row for the rest. */
export function Pointers({ list }: { list: readonly Pointer[] }) {
  if (list.length === 0) return null
  return (
    <ol className="flex flex-col gap-3">
      {list.map((p, i) => (
        <li key={`${p.kind}:${p.ref}:${i}`}>
          <PointerBlock p={p} />
        </li>
      ))}
    </ol>
  )
}

function PointerBlock({ p }: { p: Pointer }) {
  if (p.kind === 'code') {
    const ref = parseCodeRef(p.ref)
    if (ref && p.text !== null) {
      return (
        <Titled p={p}>
          <Excerpt path={ref.path} start={ref.start} text={p.text} lang={extOf(ref.path) || undefined} caption={p.note ?? undefined} />
        </Titled>
      )
    }
  }
  if (p.kind === 'diff' && p.text !== null) {
    return (
      <Titled p={p} note>
        <UnifiedDiff text={p.text} />
      </Titled>
    )
  }
  if (p.kind === 'artifact') return <ArtifactPointer p={p} />
  if (p.kind === 'url') {
    return (
      <button type="button" onClick={() => openExternal(p.ref)} className="w-full text-left">
        <Row icon={ExternalLink} title={p.title} note={p.note} meta={urlHost(p.ref)} />
      </button>
    )
  }
  const route = pointerRoute(p)
  if (!route) {
    // A `code` ref that does not parse or a `diff` without text: the row still names what the agent meant.
    return <Row icon={ICON[p.kind] ?? ListChecks} title={p.title} note={p.note} meta={p.ref} />
  }
  return (
    <Link {...route} className="block">
      <Row icon={ICON[p.kind] ?? ListChecks} title={p.title} note={p.note} meta={p.ref} />
    </Link>
  )
}

/** `title` over the evidence; `note` under it unless the evidence shows it itself (`Excerpt`'s caption). */
function Titled({ p, note, children }: { p: Pointer; note?: boolean; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-sm font-medium">{p.title}</p>
      {children}
      {note && p.note && <p className="text-xs text-fg-secondary">{p.note}</p>}
    </div>
  )
}

function Row({ icon: Icon, title, note, meta }: { icon: LucideIcon; title: string; note: string | null; meta: string }) {
  return (
    <div className="flex items-start gap-3 rounded-md border border-border-subtle px-3 py-2 text-sm transition-colors hover:bg-accent/60">
      <Icon className="mt-0.5 size-4 shrink-0 text-fg-tertiary" />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="min-w-0 flex-1 truncate font-medium">{title}</span>
          <span className="shrink-0 truncate text-xs text-fg-tertiary">{meta}</span>
        </div>
        {note && <p className="text-xs text-fg-secondary">{note}</p>}
      </div>
    </div>
  )
}

/** The artifact record the viewer route fetches (`Viewer.tsx`), rendered inline and capped, with Open → `/artifacts/$callId`. */
function ArtifactPointer({ p }: { p: Pointer }) {
  const api = useApi()
  const record = useQuery({ queryKey: queryKeys.artifact(p.ref), queryFn: () => api.get_artifact(p.ref) })
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 truncate text-sm font-medium">{p.title}</p>
        <Button asChild variant="outline" size="xs">
          <Link to="/artifacts/$callId" params={{ callId: p.ref }}>
            Open
          </Link>
        </Button>
      </div>
      {record.isPending ? (
        <Skeleton className="h-40 w-full rounded-md" />
      ) : record.isError ? (
        <Row icon={FolderOpen} title={record.error.message} note={null} meta={p.ref} />
      ) : (
        <div className="overflow-hidden rounded-md border">
          <ArtifactView artifact={record.data} className="max-h-[480px] overflow-auto" />
        </div>
      )}
      {p.note && <p className="text-xs text-fg-secondary">{p.note}</p>}
    </div>
  )
}
