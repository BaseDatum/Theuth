import { requirements as agentsRequirements } from '@/components/agents/requirements'
import { requirements as artifactsRequirements } from '@/components/artifacts/requirements'
import { requirements as attentionRequirements } from '@/components/attention/requirements'
import { requirements as autoresearchRequirements } from '@/components/autoresearch/requirements'
import { requirements as costsRequirements } from '@/components/costs/requirements'
import { requirements as knowledgeRequirements } from '@/components/knowledge/requirements'
import { secretsRequirements, skillsRequirements } from '@/components/library/requirements'
import { requirements as meetingRequirements } from '@/components/meeting/requirements'
import { requirements as notebooksRequirements } from '@/components/notebooks/requirements'
import { requirements as notesRequirements } from '@/components/notes/requirements'
import { requirements as routinesRequirements } from '@/components/routines/requirements'
import { requirements as tasksRequirements } from '@/components/tasks/requirements'
import { requirements as projectsRequirements } from '@/components/projects/requirements'
import { requirements as whiteboardRequirements } from '@/components/whiteboard/requirements'
import { requirements as deckRequirements } from '@/components/decks/requirements'
import { placementsOf, pluginStore } from '@/lib/plugins'
import { type Requirements, supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

export type ViewCatalogEntry = {
  path: string
  title: string
  group: string
  description: string
  detail_template?: string
}

type BuiltinViewSpec = ViewCatalogEntry & { required?: Requirements }

const BUILTIN_VIEWS: readonly BuiltinViewSpec[] = [
  {
    path: '/attention',
    title: 'Inbox',
    group: 'inbox',
    description: 'Pending approvals, questions, and attention items across sessions',
    detail_template: '/attention/<id>',
    required: attentionRequirements,
  },
  {
    path: '/agents',
    title: 'Agents',
    group: 'agents',
    description: 'Roster of configured agents, roles, and desks',
    detail_template: '/agents/<name>',
    required: agentsRequirements,
  },
  {
    path: '/projects',
    title: 'Projects',
    group: 'projects',
    description: 'Projects grouping chats, tasks, and notes, with description and metadata',
    detail_template: '/projects/<name>',
    required: projectsRequirements,
  },
  {
    path: '/tasks',
    title: 'Tasks',
    group: 'projects',
    description: 'Task board of a project tracking backlog, active work, and reviews (?project=<name>)',
    detail_template: '/tasks/<id>',
    required: tasksRequirements,
  },
  {
    path: '/tasks/graph',
    title: 'Task graph',
    group: 'projects',
    description: 'Dependency graph view of project tasks',
    required: tasksRequirements,
  },
  {
    path: '/routines',
    title: 'Routines',
    group: 'agents',
    description: 'Scheduled prompts, watchers, and recurring loops',
    detail_template: '/routines/<name>',
    required: routinesRequirements,
  },
  {
    path: '/autoresearch',
    title: 'Autoresearch',
    group: 'agents',
    description: 'Automated benchmark experiment studies and results',
    detail_template: '/autoresearch/<slug>',
    required: autoresearchRequirements,
  },
  {
    path: '/artifacts',
    title: 'Artifacts',
    group: 'library',
    description: 'Saved files, pages, PDFs, charts, and interactive apps',
    detail_template: '/artifacts/<callId>',
    required: artifactsRequirements,
  },
  {
    path: '/knowledge',
    title: 'Knowledge',
    group: 'library',
    description: 'Durable knowledge base concepts and revision history',
    detail_template: '/knowledge/<concept>',
    required: knowledgeRequirements,
  },
  {
    path: '/skills',
    title: 'Skills',
    group: 'library',
    description: 'Installed reusable skills and skill repositories',
    detail_template: '/skills/<name>',
    required: skillsRequirements,
  },
  {
    path: '/notes',
    title: 'Notes',
    group: 'library',
    description: 'Notes, boards, and collaborative block documents',
    detail_template: '/notes/<id>',
    required: notesRequirements,
  },
  {
    path: '/whiteboards',
    title: 'Whiteboards',
    group: 'library',
    description: 'Visual whiteboards with cards, frames, links, and marks',
    detail_template: '/whiteboards/<id>',
    required: whiteboardRequirements,
  },
  {
    path: '/decks',
    title: 'Decks',
    group: 'library',
    description: 'Collaborative live Decks with review-gated Monty streams and interactive sections',
    detail_template: '/decks/<id>',
    required: deckRequirements,
  },
  {
    path: '/notebooks',
    title: 'Notebooks',
    group: 'library',
    description: 'Research notebooks over sources, notes, and overviews',
    detail_template: '/notebooks/<id>',
    required: notebooksRequirements,
  },
  {
    path: '/meetings',
    title: 'Meetings',
    group: 'library',
    description: 'Recorded meetings, transcripts, notes, and cached briefs',
    detail_template: '/meetings/<id>',
    required: meetingRequirements,
  },
  {
    path: '/meetings/new',
    title: 'New meeting',
    group: 'library',
    description: 'Start a new live meeting recording',
    required: meetingRequirements,
  },
  {
    path: '/secrets',
    title: 'Secrets',
    group: 'library',
    description: 'Encrypted secrets vault entries and versions',
    required: secretsRequirements,
  },
  {
    path: '/costs',
    title: 'Costs',
    group: 'costs',
    description: 'Token usage and cost breakdown dashboard',
    required: costsRequirements,
  },
  {
    path: '/settings/appearance',
    title: 'Settings',
    group: 'settings',
    description: 'Application, model, provider, tool, and mesh settings',
    detail_template: '/settings/<tab>',
  },
]

function stripRequired({ required: _required, ...entry }: BuiltinViewSpec): ViewCatalogEntry {
  return entry
}

/**
 * Live catalog of app views an agent may open with `show_view`.
 * Filters built-in views by engine capabilities when discovery is ready; falls back to all built-in entries when
 * discovery is not yet ready/connected (e.g. unit tests). Appends active plugin sidebar views when loaded.
 */
export function availableViews(): ViewCatalogEntry[] {
  const discovery = useConnection.getState().discovery
  const filterByCap = discovery?.phase === 'ready'
  const out: ViewCatalogEntry[] = []

  for (const spec of BUILTIN_VIEWS) {
    if (filterByCap && spec.required && !supports(discovery, spec.required)) continue
    out.push(stripRequired(spec))
  }

  try {
    const { plugins, status } = pluginStore().getState()
    const entries = placementsOf(plugins, status, 'sidebar')
    for (const e of entries) {
      const surface = (e.placement.open === 'tile' && e.plugin.placements.find((p) => p.kind === 'tile')) || e.placement
      const props = JSON.stringify({ plugin: e.plugin.name, placement: surface.id })
      out.push({
        path: `/tile/plugin/_?props=${encodeURIComponent(props)}`,
        title: e.placement.label,
        group: 'plugins',
        description: `Plugin ${e.plugin.name}: ${e.placement.label}`,
      })
    }
  } catch {
    // Plugin store unavailable or uninitialised: skip gracefully.
  }

  return out
}

const VALID_EXACT_PATHS = new Set<string>([
  '/attention',
  '/agents',
  '/agents/new',
  '/projects',
  '/tasks',
  '/tasks/graph',
  '/routines',
  '/routines/new',
  '/autoresearch',
  '/autoresearch/new',
  '/artifacts',
  '/knowledge',
  '/skills',
  '/notes',
  '/whiteboards',
  '/decks',
  '/notebooks',
  '/meetings',
  '/meetings/new',
  '/meetings/briefs',
  '/secrets',
  '/costs',
  '/settings',
  '/library',
  '/tools',
])

const VALID_PARAM_PATTERNS: readonly RegExp[] = [
  /^\/projects\/[^/]+$/,
  /^\/tasks\/[^/]+$/,
  /^\/notebooks\/[^/]+$/,
  /^\/notes\/[^/]+$/,
  /^\/agents\/[^/]+(?:\/[^/]+)?$/,
  /^\/routines\/[^/]+(?:\/[^/]+)?$/,
  /^\/meetings\/[^/]+$/,
  /^\/autoresearch\/[^/]+$/,
  /^\/whiteboards\/[^/]+$/,
  /^\/decks\/[^/]+$/,
  /^\/knowledge\/.+$/,
  /^\/artifacts\/[^/]+$/,
  /^\/attention\/[^/]+$/,
  /^\/skills\/[^/]+$/,
  /^\/sources\/[^/]+$/,
  /^\/settings\/[^/]+$/,
  /^\/tile\/plugin\/[^/]+$/,
]

const UNKNOWN_VIEW_HINT =
  'call pane_list() for available_views (e.g. /projects, /tasks, /notebooks, /notes, /knowledge, /agents, /routines, /meetings, /whiteboards, /decks, /artifacts, /autoresearch, /skills, /attention, /costs, /settings/appearance)'

/**
 * Validate a candidate view path against the app's known routes so agents never open a 404 pane.
 */
export function validateViewPath(raw: string): { ok: true; normalized: string } | { ok: false; reason: string } {
  const trimmed = raw.trim()
  if (!trimmed || !trimmed.startsWith('/') || trimmed.includes('..') || /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(trimmed)) {
    return { ok: false, reason: `unknown view path '${raw}'; ${UNKNOWN_VIEW_HINT}` }
  }
  const normalized = trimmed
  const qIdx = normalized.search(/[?#]/)
  const rawPathname = qIdx >= 0 ? normalized.slice(0, qIdx) : normalized
  const suffix = qIdx >= 0 ? normalized.slice(qIdx) : ''
  const pathname = rawPathname.length > 1 && rawPathname.endsWith('/') ? rawPathname.slice(0, -1) : rawPathname

  const matches = VALID_EXACT_PATHS.has(pathname) || VALID_PARAM_PATTERNS.some((re) => re.test(pathname))
  if (!matches) {
    return { ok: false, reason: `unknown view path '${raw}'; ${UNKNOWN_VIEW_HINT}` }
  }
  return { ok: true, normalized: `${pathname}${suffix}` }
}
