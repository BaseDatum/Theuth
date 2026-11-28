// 215 value surface: the Library overview strip under the tab band — one tile per tab with what it holds and how much.
// Shown while the getting-started checklist is up, or while the open tab is empty (when an explanation helps most);
// only in the local desktop shell. Counts come from lists that are cheap or already cached; nothing here polls.

import { useQuery } from '@tanstack/react-query'
import { useNavigate, useRouterState } from '@tanstack/react-router'
import { useEffect } from 'react'
import { decksKey } from '@/components/decks/useDeckLive'
import type { DeckSummary } from '@/components/decks/types'
import { useNotebooks } from '@/components/notebooks/useNotebooks'
import { useNotes } from '@/components/notes/useNotes'
import { useChecklistShown, useGuidesHere } from '@/components/onboarding/visibility'
import { GROUPS, type Tab, tabOf } from '@/lib/nav'
import { useOnboarding } from '@/lib/onboarding/state'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

/** One line per tab: what it is for. */
export const LIBRARY_PURPOSE: Readonly<Partial<Record<Tab, string>>> = {
  '/artifacts': 'Pages, files and images agents made',
  '/knowledge': 'What agents learned, as linked notes',
  '/skills': 'Playbooks agents follow for a task',
  '/notes': 'Your notes and boards, shared with agents',
  '/decks': 'Live dashboards with review-gated streams',
  '/notebooks': 'Sources and chats around one topic',
  '/meetings': 'Recordings, transcripts and briefs',
  '/secrets': 'Keys and tokens agents may use',
}

/**
 * Item counts per tab; `undefined` = not known. Every list is read from the cache its own page fills; the small ones
 * (skills, notes, notebooks) are fetched too, but only while the strip is up (`fetch`).
 */
function useCounts(fetch: boolean): Partial<Record<Tab, number>> {
  const api = useApi()
  const canSkills = useQuerySupport(['list_skills'])
  // The same keys and functions as the pages' own queries, so a page refetch never runs anything else.
  const artifacts = useQuery({ queryKey: queryKeys.artifacts, queryFn: () => api.list_artifacts(null, null), enabled: false }).data
  const knowledge = useQuery({ queryKey: queryKeys.knowledgeList, queryFn: () => api.knowledge_list(null, null, null, 100000), enabled: false }).data
  const secrets = useQuery({ queryKey: queryKeys.secrets, queryFn: () => api.list_secrets(), enabled: false }).data
  const decks = useQuery<DeckSummary[]>({ queryKey: decksKey, queryFn: async () => [], enabled: false }).data
  const skills = useQuery({ enabled: fetch && canSkills, queryKey: queryKeys.skills, queryFn: () => api.list_skills() }).data
  const notes = useNotes(fetch).data
  const notebooks = useNotebooks(false, fetch).data
  return {
    '/artifacts': artifacts?.length,
    '/knowledge': knowledge?.length,
    '/secrets': secrets?.secrets.length,
    '/decks': decks?.length,
    '/skills': skills?.length,
    '/notes': notes?.length,
    '/notebooks': notebooks?.length,
  }
}

/** Marks the Library visited; in the desktop shell, on a tab's list page (not a note or notebook open under the band), the strip. */
export function LibraryOverview() {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const here = useGuidesHere()
  useEffect(() => useOnboarding.getState().markVisited('library'), [])
  if (!here) return null
  const current = tabOf('library', pathname)
  return current !== null && current === pathname.replace(/(.)\/$/, '$1') ? <Strip current={current as Tab} /> : null
}

function Strip({ current }: { current: Tab }) {
  const discovery = useConnection((s) => s.discovery)
  const navigate = useNavigate()
  const shown = useChecklistShown()
  const counts = useCounts(shown)
  if (!shown && counts[current] !== 0) return null
  const tabs = GROUPS.library.filter((t) => supports(discovery, t.required))
  if (!tabs.length) return null
  return (
    <nav
      aria-label="Library overview"
      data-tour="library-overview"
      className="hidden shrink-0 gap-2 border-b border-border-subtle px-3 py-2.5 md:grid md:[grid-template-columns:repeat(auto-fit,minmax(150px,1fr))]"
    >
      {tabs.map(({ to, label, icon: Icon }) => {
        const n = counts[to]
        const here = to === current
        return (
          <button
            key={to}
            type="button"
            aria-current={here ? 'page' : undefined}
            onClick={() => void navigate({ to })}
            className={cn(
              'flex min-w-0 flex-col gap-1 rounded-[10px] border px-3 py-2 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/50 motion-reduce:transition-none',
              here ? 'border-border-strong bg-elevated' : 'border-border-subtle hover:bg-layer-1',
            )}
          >
            <span className="flex items-center gap-1.5 text-[13px] font-medium">
              <Icon aria-hidden className={cn('size-3.5 shrink-0', here ? 'text-[var(--fg-accent)]' : 'text-fg-tertiary')} />
              <span className="truncate">{label}</span>
              {n !== undefined && <span className="ml-auto text-xs font-normal text-fg-tertiary tabular-nums">{n}</span>}
            </span>
            <span className="line-clamp-2 text-xs leading-snug text-fg-tertiary">{LIBRARY_PURPOSE[to]}</span>
          </button>
        )
      })}
    </nav>
  )
}
