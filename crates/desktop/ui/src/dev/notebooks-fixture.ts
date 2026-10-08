// 144 W4: the notebook RPCs answered from memory so the notebook page, source page and Library tab render against an
// engine that predates them (screenshots, never Sky's live engine for writes). Dev only, behind
// `localStorage['notebooks-fixture'] = "1"`. Notes are the engine's own (read-only `doc_list`), so the Notes column
// has real titles; everything else is the mock's data (`docs/design/assets/144-notebook-page.html`).

import type { Api } from '@/rpc/api'
import type { DocKind } from '@/rpc/bindings/DocKind'
import type { DocSummary } from '@/rpc/bindings/DocSummary'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { NotebooksApi } from '@/lib/notebooks-rpc'
import type { Citation, Notebook, NotebookItem, Source, SourceOrigin, SourceStatus } from '@/lib/notebooks-types'

export const notebooksFixtureOn = (): boolean => import.meta.env.DEV && globalThis.localStorage?.getItem('notebooks-fixture') === '1'

const NOW = Date.now()
const DAY = 86_400_000
const ms = (daysAgo: number) => BigInt(NOW - daysAgo * DAY)

const doc = (id: string, kind: string, title: string, daysAgo: number): DocSummary => ({
  id,
  kind: kind as DocKind,
  title,
  binding: 'note',
  created_ms: ms(daysAgo + 2),
  updated_ms: ms(daysAgo),
  bytes: 4096n,
})

export const NB_ID = 'fx-nb-dr'
const NB2_ID = 'fx-nb-onboarding'

const notebooks: Notebook[] = [
  { doc: doc(NB_ID, 'notebook', 'DR verification — ICD portal', 0.1), project: 'dr-2026', archived_ms: null, sources: 7, notes: 3, chats: 3 },
  { doc: doc(NB2_ID, 'notebook', 'Onboarding — platform team', 4), project: null, archived_ms: null, sources: 2, notes: 1, chats: 0 },
  { doc: doc('fx-nb-old', 'notebook', 'Q2 incident retro', 60), project: 'sre', archived_ms: ms(30), sources: 3, notes: 2, chats: 1 },
]

const descriptions: Record<string, string> = {
  [NB_ID]: 'Prove the DR path for the ICD portal end to end; every claim cited to a runbook, a meeting or a ticket.\n\nScope: portal tier, reporting database, Route 53 failover.',
  [NB2_ID]: 'What a new platform engineer reads in week one.',
  'fx-nb-old': 'Retro notes for the Q2 incidents.',
}

const src = (id: string, title: string, origin: SourceOrigin, status: SourceStatus, extra: Partial<Source> = {}): Source => ({
  doc: doc(id, 'source', title, 3),
  origin,
  status,
  progress: null,
  error: null,
  attempts: 1,
  fetched_ms: ms(3),
  content_sha: 'a3f9c0de1b22',
  changed_since: null,
  bytes: 120_000n,
  words: 18_000,
  duration_ms: null,
  pages: null,
  insights: 0,
  meta: {},
  ...extra,
})

const sources: Source[] = [
  src('fx-s-runbook', 'ICD portal DR runbook v7', { kind: 'file', name: 'icd-dr-runbook-v7.pdf', sha256: 'aa', ext: 'pdf' }, 'completed', { pages: 48, insights: 1 }),
  src('fx-s-r53', 'AWS Route 53 failover docs', { kind: 'url', url: 'https://docs.aws.amazon.com/route53/failover', canonical: null }, 'completed', { fetched_ms: ms(9), changed_since: ms(1) }),
  src('fx-s-mtg', 'DR review with platform team', { kind: 'meeting', meeting: 'm-2026-10-02' }, 'completed', { duration_ms: 54n * 60_000n, fetched_ms: ms(6) }),
  src('fx-s-jira', 'DR-412 Failover drill results', { kind: 'mcp', server: 'jira', tool: 'get_issue', args: { key: 'DR-412' }, label: 'jira' }, 'completed', { fetched_ms: ms(1) }),
  src('fx-s-yt', 're:Invent — multi-region failover', { kind: 'youtube', video: 'dQw4w9WgXcQ', url: 'https://youtu.be/dQw4w9WgXcQ', route: 'audio' }, 'running', { progress: 62, duration_ms: 41n * 60_000n, fetched_ms: null }),
  src('fx-s-ses', 'Session: “why did RDS promote late”', { kind: 'session', session: '01a10e31-47cf-7103-be29-f5a4983e45a3' }, 'completed', { fetched_ms: ms(8) }),
  src('fx-s-aud', 'voice memo — post-drill notes.m4a', { kind: 'audio', name: 'post-drill notes.m4a', sha256: 'bb', ext: 'm4a' }, 'failed', { error: 'unsupported codec (415)', attempts: 3, duration_ms: 6n * 60_000n, fetched_ms: null }),
  src('fx-s-onb', 'Platform handbook', { kind: 'url', url: 'https://wiki.example.com/platform/handbook', canonical: null }, 'completed'),
  src('fx-s-onb2', 'Welcome deck.pdf', { kind: 'file', name: 'Welcome deck.pdf', sha256: 'cc', ext: 'pdf' }, 'queued', { fetched_ms: null }),
]
const sourceNotebook: Record<string, string> = Object.fromEntries(sources.map((s) => [s.doc.id, s.doc.id.startsWith('fx-s-onb') ? NB2_ID : NB_ID]))

let items: NotebookItem[] = sources.map((s) => ({ notebook: sourceNotebook[s.doc.id]!, kind: 'source', item: s.doc.id, added_ms: ms(3), added_by: 'owner', removed_ms: null }))
let notesSeeded = false

const chats: SessionSummary[] = []

/** The notebook RPCs from memory; `api` is the engine's (read-only here: `doc_list` for real note ids, `list_sessions` for chats). */
export function notebooksFixture(api: Api): NotebooksApi {
  const seedNotes = async () => {
    if (notesSeeded) return
    notesSeeded = true
    const notes = await api.doc_list('note', 6).catch(() => [] as DocSummary[])
    notes.forEach((n, i) => items.push({ notebook: i < 3 ? NB_ID : NB2_ID, kind: 'note', item: n.id, added_ms: ms(i), added_by: 'owner', removed_ms: null }))
    const sessions = await api.list_sessions().catch(() => [] as SessionSummary[])
    chats.push(...sessions.filter((s) => s.parent === null).slice(0, 3))
  }
  const find = (id: string) => notebooks.find((n) => n.doc.id === id) ?? Promise.reject(new Error(`no notebook ${id}`))
  const ok = () => Promise.resolve(null)
  return {
    notebook_list: (archived) => Promise.resolve(notebooks.filter((n) => (n.archived_ms !== null) === archived)),
    notebook_get: (id) => Promise.resolve(find(id)),
    notebook_create: (title, _d, project) => {
      const n: Notebook = { doc: doc(`fx-nb-${Date.now()}`, 'notebook', title, 0), project, archived_ms: null, sources: 0, notes: 0, chats: 0 }
      notebooks.unshift(n)
      return Promise.resolve(n)
    },
    notebook_update: async (id, title, project, archived) => {
      const n = await find(id)
      if (title !== null) n.doc.title = title
      if (project !== null) n.project = project
      if (archived !== null) n.archived_ms = archived ? ms(0) : null
      return n
    },
    notebook_delete_preview: () => Promise.resolve({ notes: 3, sources: 7, exclusive_sources: 5, chats: 3 }),
    notebook_delete: (id) => {
      notebooks.splice(notebooks.findIndex((n) => n.doc.id === id), 1)
      return ok()
    },
    notebook_items_list: async (nb) => {
      await seedNotes()
      return items.filter((i) => i.notebook === nb)
    },
    notebook_item_add: (notebook, kind, item) => {
      const row: NotebookItem = { notebook, kind, item, added_ms: ms(0), added_by: 'owner', removed_ms: null }
      items.push(row)
      return Promise.resolve(row)
    },
    notebook_item_remove: (notebook, kind, item) => {
      items = items.filter((i) => !(i.notebook === notebook && i.kind === kind && i.item === item))
      return ok()
    },
    notebook_chats: async () => {
      await seedNotes()
      return chats
    },
    session_set_notebook: ok,
    source_add: (notebook, spec, title) => {
      const origin: SourceOrigin =
        'url' in spec ? { kind: 'url', url: spec.url, canonical: null } : 'text' in spec ? { kind: 'text', title: title ?? 'Pasted text', sha256: 'dd' } : { kind: 'file', name: title ?? 'file', sha256: 'ee', ext: 'bin' }
      const s = src(`fx-s-${Date.now()}`, title ?? ('url' in spec ? spec.url : 'New source'), origin, 'queued', { fetched_ms: null })
      sources.push(s)
      sourceNotebook[s.doc.id] = notebook
      items.push({ notebook, kind: 'source', item: s.doc.id, added_ms: ms(0), added_by: 'owner', removed_ms: null })
      return Promise.resolve(s)
    },
    source_get: (id) => Promise.resolve(sources.find((s) => s.doc.id === id) ?? Promise.reject(new Error(`no source ${id}`))),
    source_list: (nb) => Promise.resolve(sources.filter((s) => nb === null || sourceNotebook[s.doc.id] === nb)),
    source_refresh: async (id) => {
      const s = sources.find((x) => x.doc.id === id)!
      s.status = 'queued'
      s.changed_since = null
      s.error = null
      return s
    },
    source_insights_run: () => Promise.resolve([]),
    insight_promote: () => Promise.reject(new Error('wave 2')),
    notebook_context_set: ok,
    notebook_context_estimate: () => Promise.resolve({ items: [], tokens: 12_400, budget: 40_000, omitted: 1 }),
    cite_resolve: (hrefs) =>
      Promise.resolve(
        hrefs.map((href): Citation => {
          const m = /^theuth:\/\/(\w+)\/([^#@?]+)([#@][^?]*)?(?:\?s=(\w+))?$/.exec(href)
          const id = m?.[2] ?? ''
          const s = sources.find((x) => x.doc.id === id)
          return {
            href,
            kind: (m?.[1] ?? 'notes') as Citation['kind'],
            id,
            locator: m?.[3] ?? null,
            sha8: m?.[4] ?? null,
            status: href.includes('moved') ? 'moved' : 'same',
            available: !href.includes('DR-398'),
            title: s?.doc.title ?? (m?.[1] === 'notes' ? 'Drill timeline' : 'Removed source'),
            text: null,
          }
        }),
      ),
    save_answer_as_note: () => Promise.reject(new Error('fixture: no writes')),
    promote_answer: () => Promise.reject(new Error('fixture: no writes')),
  }
}

/** The notebook document's markdown (its description) for a fixture id; null for anything real. */
export const fixtureMarkdown = (id: string): string | null => (notebooksFixtureOn() ? (descriptions[id] ?? null) : null)
