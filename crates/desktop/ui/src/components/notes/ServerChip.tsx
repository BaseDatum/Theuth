import { useEffect, useState } from 'react'
import { type Mesh, inMesh, machineName } from '@/components/notes/machine'
import { useMesh } from '@/components/tasks/queries'
import type { EngineProvider } from '@/lib/doc/provider'
import type { DocKind } from '@/rpc/bindings/DocKind'
import { cn } from '@/lib/utils'

export type ServerView = { name: string; online: boolean; title: string; banner: { bold: string; rest: string } | null }

/** 184 §12.1: the chip and banner of a note, board or meeting note in a mesh; null when there is nothing to say (no mesh, unknown server, not a note kind). Unhomed: the server is the holder. */
export function serverView(mesh: Mesh, kind: DocKind | undefined, home: string | null, server: string | null, dual: string | null): ServerView | null {
  if (!inMesh(mesh) || !server || (kind !== 'note' && kind !== 'board' && kind !== 'meeting_note')) return null
  const name = machineName(mesh, home ?? server)
  const title = `This note lives on ${name}; other machines edit it through it.`
  if (dual) return { name, online: true, title, banner: { bold: `Also open on ${machineName(mesh, dual)}`, rest: ' — edits merge within seconds.' } }
  if (home && server !== home) {
    return { name, online: false, title, banner: { bold: `${name} is offline`, rest: ` — editing on ${machineName(mesh, server)} for now. Your changes merge back when ${name} returns.` } }
  }
  return { name, online: true, title, banner: null }
}

/** The view of `provider`'s document, re-read on every `Server` event and every re-open (status change). */
export function useServerView(provider: EngineProvider): ServerView | null {
  const mesh = useMesh().data
  const [, tick] = useState(0)
  useEffect(() => {
    const bump = () => tick((n) => n + 1)
    const offs = [provider.onEvent((e) => e.type === 'Server' && bump()), provider.onStatus(bump)]
    return () => offs.forEach((off) => off())
  }, [provider])
  if (provider.status.kind === 'closed') return null
  return serverView(mesh, provider.meta?.kind, provider.meta?.home ?? null, provider.server, provider.dual)
}

export function ServerChip({ provider }: { provider: EngineProvider }) {
  const v = useServerView(provider)
  if (!v) return null
  return (
    <span title={v.title} className="mr-2 flex max-w-48 min-w-0 items-center gap-1.5 text-xs text-fg-secondary">
      <span className={cn('size-1.5 shrink-0 rounded-full', v.online ? 'bg-success' : 'bg-fg-tertiary')} />
      <span className="truncate">
        On {v.name}
        {!v.online && ' · offline'}
      </span>
    </span>
  )
}

export function ServerBanner({ provider }: { provider: EngineProvider }) {
  const b = useServerView(provider)?.banner
  if (!b) return null
  return (
    <p role="status" className="mx-4 mt-2 rounded-md border border-warning-border px-3 py-2 text-[13px]">
      <strong>{b.bold}</strong>
      {b.rest}
    </p>
  )
}
