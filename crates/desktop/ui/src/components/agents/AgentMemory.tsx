import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { ReadOnlyDocument } from '@/components/knowledge/Document'
import { Input } from '@/components/ui/input'
import { getMemory, listMemory, searchMemory } from '@/lib/agent-memory'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'

/** Owner-only, read-only view. Bundle and id both belong in the cache key, never in main memory's cache. */
export function AgentMemory({ name }: { name: string }) {
  const api = useApi()
  const [query, setQuery] = useState('')
  const [search, setSearch] = useState('')
  const [selected, setSelected] = useState<string | null>(null)
  useEffect(() => {
    const timer = setTimeout(() => setSearch(query.trim()), 200)
    return () => clearTimeout(timer)
  }, [query])
  const notes = useQuery({
    queryKey: ['knowledge', 'agent-memory', name, 'list', search],
    queryFn: async () => search ? (await searchMemory(api, name, search)).map((hit) => hit.meta) : listMemory(api, name),
    retry: false,
  })
  const preview = useQuery({
    queryKey: ['knowledge', 'agent-memory', name, 'doc', selected],
    queryFn: () => getMemory(api, name, selected!),
    enabled: selected !== null,
    retry: false,
  })
  return (
    <section aria-label="Agent memory" className="grid min-h-full min-w-0 md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]">
      <div className="min-w-0 border-b border-border-subtle md:border-r md:border-b-0">
        <div className="p-3">
          <Input aria-label="Search this agent's memory" placeholder="Search this agent's memory…" value={query} onChange={(e) => { setQuery(e.target.value); setSelected(null) }} />
        </div>
        {notes.isPending && <p className="px-3 py-2 text-sm text-fg-tertiary">Loading…</p>}
        {notes.isError && <p role="alert" className="px-3 py-2 text-sm text-danger">{notes.error.message}</p>}
        {notes.data?.length === 0 && <p className="px-3 py-6 text-center text-sm text-fg-tertiary">{search ? 'No matches' : 'Nothing remembered yet. What this agent records stays here and never reaches your main memory.'}</p>}
        <ul>
          {notes.data?.map((note) => (
            <li key={note.id}>
              <button type="button" aria-pressed={selected === note.id} onClick={() => setSelected(note.id)} className={cn('block w-full border-t border-border-subtle px-3 py-2 text-left hover:bg-layer-2', selected === note.id && 'bg-layer-2')}>
                <span className="block break-words text-sm">{note.title || note.id}</span>
                <span className="block break-all font-mono text-xs text-fg-tertiary">{note.id}</span>
              </button>
            </li>
          ))}
        </ul>
        {notes.data && <footer className="px-3 py-3 text-xs text-fg-tertiary">{notes.data.length} notes · stored on this machine only · read-only here</footer>}
      </div>
      <div className="min-w-0 p-3 md:p-0" aria-label="Memory preview">
        {selected && preview.isPending && <p className="p-3 text-sm text-fg-tertiary">Loading…</p>}
        {selected && preview.isError && <p role="alert" className="p-3 text-sm text-danger">{preview.error.message}</p>}
        {selected && preview.data && <ReadOnlyDocument concept={preview.data} onSelect={setSelected} />}
        {!selected && notes.data && notes.data.length > 0 && <p className="p-3 text-sm text-fg-tertiary">Select a note to preview it.</p>}
      </div>
    </section>
  )
}
