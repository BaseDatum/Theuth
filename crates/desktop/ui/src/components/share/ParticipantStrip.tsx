import { useEffect, useState } from 'react'
import type { HarnessProvider } from '@/lib/doc/provider'
import { cn } from 'cn'

/** One awareness client with a `user` (59 §8.5): the owner's devices, members and, while it edits, the agent. */
export type Participant = { id: number; name: string; color: string; role?: string; me: boolean }

/** Every awareness state of `provider` that names a user, this client first, agent states excluded (the footer chip has those). */
export function participants(provider: HarnessProvider): Participant[] {
  const out: Participant[] = []
  for (const [id, state] of provider.awareness.getStates()) {
    if (!state || typeof state !== 'object' || 'agent' in state || !('user' in state)) continue
    const user: unknown = state.user
    if (!user || typeof user !== 'object' || !('name' in user) || !('color' in user)) continue
    if (typeof user.name !== 'string' || typeof user.color !== 'string') continue
    const role = 'role' in state && typeof state.role === 'string' ? state.role : undefined
    out.push({ id, name: user.name, color: user.color, role, me: id === provider.doc.clientID })
  }
  return out.sort((a, b) => Number(b.me) - Number(a.me) || a.id - b.id)
}

/** `participants(provider)`, re-read on every awareness change. */
export function useParticipants(provider: HarnessProvider): Participant[] {
  const [list, setList] = useState<Participant[]>(() => participants(provider))
  useEffect(() => {
    const read = () => setList(participants(provider))
    read()
    provider.awareness.on('change', read)
    return () => provider.awareness.off('change', read)
  }, [provider])
  return list
}

/** Colour dots + names of everyone in the document (59 §8.5), in the editor's header bar. */
export function ParticipantStrip({ provider, className }: { provider: HarnessProvider; className?: string }) {
  const list = useParticipants(provider)
  if (list.length === 0) return null
  return (
    <ul className={cn('flex min-w-0 items-center gap-2 overflow-hidden text-xs text-fg-secondary', className)} aria-label="Participants">
      {list.map((p) => (
        <li key={p.id} className="flex shrink-0 items-center gap-1" title={p.role ? `${p.name} · ${p.role}` : p.name}>
          <span className="size-2 rounded-full" style={{ background: p.color }} aria-hidden />
          <span className={cn('max-w-24 truncate', p.me && 'text-fg-tertiary')}>{p.me ? `${p.name} (you)` : p.name}</span>
        </li>
      ))}
    </ul>
  )
}
