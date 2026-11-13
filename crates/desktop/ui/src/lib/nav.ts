import type { Requirements } from '@/rpc/discovery'
import { requirements as agentsRequirements } from '@/components/agents/requirements'
import { requirements as tasksRequirements } from '@/components/tasks/requirements'
import { requirements as routinesRequirements } from '@/components/routines/requirements'
import { requirements as autoresearchRequirements } from '@/components/autoresearch/requirements'
import { requirements as artifactsRequirements } from '@/components/artifacts/requirements'
import { requirements as knowledgeRequirements } from '@/components/knowledge/requirements'
import { requirements as notesRequirements } from '@/components/notes/requirements'
import { requirements as notebooksRequirements } from '@/components/notebooks/requirements'
import { requirements as meetingRequirements } from '@/components/meeting/requirements'
import { skillsRequirements, secretsRequirements } from '@/components/library/requirements'
// 97: the sidebar's two grouped buttons (Agents, Library) and the route-backed tab rows under their pages.
// URLs do not change; a tab IS a navigation to its route. Sky (2026-11-11): Skills is a Library tab and Tools is
// Settings › Tools › MCP (143 R1 had both under Agents); Agents keeps the work, Library the content.

import { BookOpen, Bot, CalendarClock, FlaskConical, FolderOpen, ListChecks, Lock, type LucideIcon, Mic, Notebook, Sparkles, StickyNote } from 'lucide-react'

export type Group = 'agents' | 'library'

/** A tab's route, as a literal so `navigate({ to })` type-checks. */
export type Tab = '/agents' | '/tasks' | '/routines' | '/artifacts' | '/knowledge' | '/notes' | '/notebooks' | '/meetings' | '/skills' | '/secrets' | '/autoresearch'

export const GROUPS: Record<Group, readonly { to: Tab; label: string; icon: LucideIcon; required: Requirements }[]> = {
  agents: [
    { required: agentsRequirements, to: '/agents', label: 'Agents', icon: Bot },
    { required: tasksRequirements, to: '/tasks', label: 'Tasks', icon: ListChecks },
    { required: routinesRequirements, to: '/routines', label: 'Routines', icon: CalendarClock },
    // 140 (Sky, 2026-10-08 23:58): the Study view sits next to Routines — a study is a routine-driven loop.
    { required: autoresearchRequirements, to: '/autoresearch', label: 'Autoresearch', icon: FlaskConical },
  ],
  // Artifacts first (Sky, 2026-09-27): the most-opened entry.
  library: [
    { required: artifactsRequirements, to: '/artifacts', label: 'Artifacts', icon: FolderOpen },
    { required: knowledgeRequirements, to: '/knowledge', label: 'Knowledge', icon: BookOpen },
    { required: skillsRequirements, to: '/skills', label: 'Skills', icon: Sparkles },
    { required: notesRequirements, to: '/notes', label: 'Notes', icon: StickyNote },
    { required: notebooksRequirements, to: '/notebooks', label: 'Notebooks', icon: Notebook },
    { required: meetingRequirements, to: '/meetings', label: 'Meetings', icon: Mic },
    { required: secretsRequirements, to: '/secrets', label: 'Secrets', icon: Lock },
  ],
}

/** The group's tab whose route is `pathname` or a prefix of it at a `/` boundary; null when none. */
export const tabOf = (group: Group, pathname: string): string | null =>
  GROUPS[group].find((t) => pathname === t.to || pathname.startsWith(`${t.to}/`))?.to ?? null

/** The group whose tab owns `pathname`; null outside both sections. */
export const groupOf = (pathname: string): Group | null =>
  tabOf('agents', pathname) ? 'agents' : tabOf('library', pathname) ? 'library' : null

export type Section = 'home' | 'talk' | 'agents' | 'attention' | 'library' | 'meeting' | 'costs' | 'settings' | null

/** Which sidebar row `pathname` lights up; null for chats and the rest. */
export const sectionOf = (pathname: string): Section => {
  if (pathname === '/') return 'home'
  if (pathname.startsWith('/talk/')) return 'talk'
  if (tabOf('agents', pathname)) return 'agents'
  if (pathname === '/attention' || pathname.startsWith('/attention/')) return 'attention'
  // The start form is the Meeting button's page, not a Library tab.
  if (pathname === '/meetings/new') return 'meeting'
  if (tabOf('library', pathname)) return 'library'
  if (pathname === '/costs' || pathname.startsWith('/costs/')) return 'costs'
  // 188: Settings is a page now, its footer button lit like any row.
  if (pathname === '/settings' || pathname.startsWith('/settings/')) return 'settings'
  return null
}
