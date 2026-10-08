// 97: the sidebar's two grouped buttons (Agents, Library) and the route-backed tab rows under their pages.
// URLs do not change; a tab IS a navigation to its route. 143 R1: Skills and Tools sit under Agents (what agents
// are made of), Library keeps the content pages.

import { BookOpen, Bot, CalendarClock, FlaskConical, FolderOpen, ListChecks, Lock, type LucideIcon, Mic, Sparkles, StickyNote, Wrench } from 'lucide-react'

export type Group = 'agents' | 'library'

/** A tab's route, as a literal so `navigate({ to })` type-checks. */
export type Tab = '/agents' | '/tasks' | '/routines' | '/artifacts' | '/knowledge' | '/notes' | '/meetings' | '/skills' | '/secrets' | '/tools' | '/autoresearch'

export const GROUPS: Record<Group, readonly { to: Tab; label: string; icon: LucideIcon }[]> = {
  agents: [
    { to: '/agents', label: 'Agents', icon: Bot },
    { to: '/tasks', label: 'Tasks', icon: ListChecks },
    { to: '/routines', label: 'Routines', icon: CalendarClock },
    { to: '/skills', label: 'Skills', icon: Sparkles },
    { to: '/tools', label: 'Tools', icon: Wrench },
  ],
  // Artifacts first (Sky, 2026-09-27): the most-opened entry.
  library: [
    { to: '/artifacts', label: 'Artifacts', icon: FolderOpen },
    { to: '/knowledge', label: 'Knowledge', icon: BookOpen },
    { to: '/notes', label: 'Notes', icon: StickyNote },
    { to: '/meetings', label: 'Meetings', icon: Mic },
    { to: '/secrets', label: 'Secrets', icon: Lock },
    // 140 R1-D4: the Study view is the last Library tab.
    { to: '/autoresearch', label: 'Autoresearch', icon: FlaskConical },
  ],
}

/** The group's tab whose route is `pathname` or a prefix of it at a `/` boundary; null when none. */
export const tabOf = (group: Group, pathname: string): string | null =>
  GROUPS[group].find((t) => pathname === t.to || pathname.startsWith(`${t.to}/`))?.to ?? null

/** The group whose tab owns `pathname`; null outside both sections. */
export const groupOf = (pathname: string): Group | null =>
  tabOf('agents', pathname) ? 'agents' : tabOf('library', pathname) ? 'library' : null

export type Section = 'home' | 'talk' | 'agents' | 'attention' | 'library' | 'meeting' | null

/** Which sidebar row `pathname` lights up; null for chats, settings, and the rest. */
export const sectionOf = (pathname: string): Section => {
  if (pathname === '/') return 'home'
  if (pathname.startsWith('/talk/')) return 'talk'
  if (tabOf('agents', pathname)) return 'agents'
  if (pathname === '/attention' || pathname.startsWith('/attention/')) return 'attention'
  // The start form is the Meeting button's page, not a Library tab.
  if (pathname === '/meetings/new') return 'meeting'
  if (tabOf('library', pathname)) return 'library'
  return null
}
