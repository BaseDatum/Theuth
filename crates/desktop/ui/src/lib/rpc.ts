import { useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import type { Api } from '@/rpc/api'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'
import { isPopout, openPopout } from '@/lib/popout'
import { applySandboxEvent } from '@/store/sandboxes'
import { endlessFixture } from '@/dev/endless-fixture'

/** The connected `Api`. Routes render only once connected, so a missing client is a bug. In dev the 137 fixture may answer the endless methods (`dev/endless-fixture.ts`). */
export function useApi(): Api {
  const api = useConnection((s) => s.api)
  if (!api) throw new Error('useApi before the connection opened')
  return import.meta.env.DEV ? endlessFixture(api) : api
}

export const queryKeys = {
  sessions: ['sessions'] as const,
  /** `agents_list()` — the Agents page roster (`components/tasks/queries` `keys.agents` aliases it); refreshed with `sessions` (100 D5). */
  roster: ['roster'] as const,
  session: (id: string) => ['session', id] as const,
  /** `session_orphans(id)` (68 §4.3); dropped on `SessionOwnerChanged`. */
  sessionOrphans: (id: string) => ['session-orphans', id] as const,
  /** `sessions_status()` (68 §10): Settings → Mesh → Sessions. */
  sessionsStatus: ['sessions-status'] as const,
  /** `list_project_styles()` (51 §3 as-built); the style dialog invalidates it after `set_project_style`. */
  projectStyles: ['project-styles'] as const,
  /** Prefix of every unified-search page (49 §4.2); invalidated by `useInvalidateOn` and the reconnect/`Lagged` reload. */
  searchAll: ['search'] as const,
  /**
   * One `search` request (49 §5.3): engine scope comes from the per-engine QueryClient; `categories` is the
   * canonical selected set; `gen` is the dialog-open generation, so a reopen never adopts a prior open's fetch.
   */
  search: (q: string, categories: readonly SearchCategory[], limit: number, gen = 0) =>
    ['search', q, categories, limit, gen] as const,
  models: ['models'] as const,
  mcp: (s: string | null) => ['mcp', s] as const,
  skills: ['skills'] as const,
  skill: (name: string) => ['skill', name] as const,
  logins: ['logins'] as const,
  providers: ['providers'] as const,
  awsProfiles: ['aws-profiles'] as const,
  acp: ['acp'] as const,
  acpRegistry: ['acp', 'registry'] as const,
  agents: (id: string) => ['agents', id] as const,
  /** `session_cost(id)` (51 §6.5); invalidated by `ModelUsageChanged`. */
  sessionCost: (id: string) => ['session-cost', id] as const,
  /** `advisor_status(id)` (125 §8.1); patched in place by `AdvisorChanged`. */
  advisorStatus: (id: string) => ['advisor-status', id] as const,
  terminals: (id: string) => ['terminals', id] as const,
  jobs: (id: string) => ['jobs', id] as const,
  prompt: (id: string) => ['prompt', id] as const,
  knowledge: ['knowledge'] as const,
  knowledgeList: ['knowledge', 'list'] as const,
  knowledgeDoc: (id: string) => ['knowledge', 'doc', id] as const,
  /** Prefix of every `knowledgeHistory` page; commits are immutable, so revisions are keyed apart and never invalidated. */
  knowledgeHistories: ['knowledge', 'history'] as const,
  knowledgeHistory: (id: string | null, limit: number) =>
    ['knowledge', 'history', id, limit] as const,
  knowledgeRevision: (commit: string, id: string | null) =>
    ['knowledge', 'revision', commit, id] as const,
  knowledgeSearch: (q: string) => ['knowledge', 'search', q] as const,
  /** `knowledge_read_revision`: commits are immutable, so never invalidated. */
  knowledgeReadRevision: (id: string, commit: string) =>
    ['knowledge', 'read-revision', id, commit] as const,
  artifacts: ['artifacts'] as const,
  artifact: (callId: string) => ['artifacts', 'get', callId] as const,
  artifactSearchMatch: (callId: string, q: string) => ['artifacts', 'match', callId, q] as const,
  /** `doc_list(note)` (59 §7.3); invalidated by `DocumentChanged`. */
  notes: ['notes'] as const,
  info: ['info'] as const,
  titleModel: ['title-model'] as const,
  subagentModel: ['subagent-model'] as const,
  /** `vision_model()` (52 §3.2); slash setters and the General dropdown invalidate this same key. */
  visionModel: ['vision-model'] as const,
  /** `escalate_model()` (57 §0.9); the Agent loop dropdown and `/escalate` set it. */
  escalateModel: ['escalate-model'] as const,
  signal: ['signal'] as const,
  slack: ['slack'] as const,
  telegram: ['telegram'] as const,
  computer: ['computer'] as const,
  secrets: ['secrets'] as const,
  updateStatus: ['update-status'] as const,
  voice: ['voice'] as const,
  /** `get_communication(session)`; `null` is the global switch. Prefix `['communication']` covers every session (children inherit, so any change invalidates all). */
  communication: (session: string | null) => ['communication', session] as const,
  peers: (session: string, query: string | null) => ['peers', session, query] as const,
  // Plain pages and InfiniteData must never share a cache entry; the session prefix invalidates both.
  peerMessages: (session: string, pendingOnly: boolean, mode: 'page' | 'infinite') =>
    ['peer-messages', session, pendingOnly, mode] as const,
  peerMessage: (session: string, id: string) => ['peer-message', session, id] as const,
  a2a: ['a2a'] as const,
  // 53 (03 §3): `meetings` is the prefix of the list and every `meeting(id)`; status/topics are the current recording.
  meetings: ['meetings'] as const,
  meeting: (id: string) => ['meetings', id] as const,
  meetingStatus: ['meeting-status'] as const,
  meetingBriefs: (id: string) => ['meeting-briefs', id] as const,
  meetingLive: (id: string) => ['meeting-live', id] as const,
  meetingProcesses: ['meeting-processes'] as const,
  meetingPermissions: ['meeting-permissions'] as const,
  // 53 (04 §7): the transcript of record and the summary of one meeting; the enrolled speaker profiles.
  meetingTranscript: (id: string) => ['meeting-transcript', id] as const,
  meetingSummary: (id: string) => ['meeting-summary', id] as const,
  meetingSpeakers: ['meeting-speakers'] as const,
  /** `search_settings_get()` (16 §8): Settings › Search providers and defaults. */
  searchSettings: ['search-settings'] as const,
  /** `search_status()` (16 §8): per-engine health from the last calls. */
  searchStatus: ['search-status'] as const,
  /** `meeting_live_diarization()` (07 §6): the Settings › Meetings switch. */
  meetingLiveDiarization: ['meeting-live-diarization'] as const,
  // 57 §3.4 / §7.1: the decision oracle — `decision_mode(prefix)` (Labs switches, the Meetings pointer, the island's
  // mode badge), `decision_training(prefix)` (Labs capture/share rows), `decision_stats()` (Labs counters).
  decisionMode: (prefix: string) => ['decision-mode', prefix] as const,
  decisionTraining: (prefix: string) => ['decision-training', prefix] as const,
  decisionStats: ['decision-stats'] as const,
  /** `decision_loaded()` (57 §2.1): the Labs header's "Decision models loaded now" line. */
  decisionLoaded: ['decision-loaded'] as const,
  // 126 §10: Settings › Double — `double_status()`, `double_classes()`, `double_class(key)`, `double_settings_get()`.
  doubleStatus: ['double', 'status'] as const,
  doubleClasses: ['double', 'classes'] as const,
  doubleClass: (key: string) => ['double', 'class', key] as const,
  doubleSettings: ['double', 'settings'] as const,
  /** Prefix of every double query; a `DoubleTrainingState` / `DoubleVersionActivated` invalidates all of it. */
  doubleAll: ['double'] as const,
  // 53 (05 §7): the brief cache — `briefs` prefixes every list/search page; a `BriefChanged` invalidates all of it.
  briefs: ['briefs'] as const,
  briefsList: (tag: string | null) => ['briefs', 'list', tag] as const,
  briefsSearch: (q: string) => ['briefs', 'search', q] as const,
  briefTags: ['briefs', 'tags'] as const,
  /** 137 §11.2 `session_memory(id)`: the header's ∞ badge and the Memory panel; refetched on `EndlessProgress`. */
  sessionMemory: (id: string) => ['session-memory', id] as const,
  /** 137 §11.1 `endless_settings_get()`: Settings → Memory and the New chat ∞ switch's initial state. */
  endlessSettings: ['endless-settings'] as const,
}

/** One `list_sessions` row with `SessionOwnerChanged` applied; the list is returned as is when the session is not in it. */
export function withOwner(
  rows: readonly SessionSummary[] | undefined,
  e: { session: string; owner: string; epoch: number; here: boolean },
): SessionSummary[] | undefined {
  if (!rows?.some((s) => s.id === e.session)) return rows as SessionSummary[] | undefined
  return rows.map((s) => (s.id === e.session ? { ...s, owner: e.owner, epoch: e.epoch, here: e.here } : s))
}

const SESSIONS_DEBOUNCE_MS = 500

/**
 * Wire events to query invalidation (25 §6.2): the session list debounced, the rest immediate. The agents roster
 * (`status`, `desk`) rides the same debounce (100 D5), so the Agents page's chips follow turns within ~0.5 s.
 */
export function useInvalidateOn(): void {
  const qc = useQueryClient()
  useEffect(() => {
    let timer: number | undefined
    const sessions = () => {
      clearTimeout(timer)
      timer = window.setTimeout(() => {
        void qc.invalidateQueries({ queryKey: queryKeys.sessions })
        void qc.invalidateQueries({ queryKey: queryKeys.roster })
      }, SESSIONS_DEBOUNCE_MS)
    }
    const search = () => void qc.invalidateQueries({ queryKey: queryKeys.searchAll })
    // The model inventory and both model settings depend on configured providers (52 §3.2): refetch
    // after an auth change and after a reconnect reload (`App.reload`), whose gap may have hidden one. A
    // settled resync (a context edit, 129's `Lagged`) touches one transcript only — nothing here.
    const models = (ev?: Event) => {
      if (ev instanceof CustomEvent && (ev.detail as { settled?: boolean } | null)?.settled) return
      void qc.invalidateQueries({ queryKey: queryKeys.models })
      void qc.invalidateQueries({ queryKey: queryKeys.titleModel })
      void qc.invalidateQueries({ queryKey: queryKeys.subagentModel })
      void qc.invalidateQueries({ queryKey: queryKeys.visionModel })
      void qc.invalidateQueries({ queryKey: queryKeys.escalateModel })
    }
    window.addEventListener('theuth:reload-session', models)
    const offs = [
      on('TurnStarted', sessions),
      on('TurnFinished', () => {
        sessions()
        search()
      }),
      on('SubagentSpawned', sessions),
      on('TitleChanged', () => {
        sessions()
        search()
      }),
      on('Compacted', search),
      on('ContextChanged', search),
      on('SessionDeleted', (e) => {
        void qc.invalidateQueries({ queryKey: queryKeys.sessions })
        void qc.invalidateQueries({ queryKey: queryKeys.roster })
        search()
        window.dispatchEvent(new CustomEvent('theuth:session-closing', { detail: e }))
      }),
      on('SessionClosing', (e) => {
        window.dispatchEvent(new CustomEvent('theuth:session-closing', { detail: e }))
      }),
      on('ModelUsageChanged', (e) => void qc.invalidateQueries({ queryKey: ['session-cost', e.session] })),
      on('SessionMetadataChanged', () => {
        sessions()
        void qc.invalidateQueries({ queryKey: queryKeys.artifacts })
      }),
      // Ownership moved (68 §4.2): patch the cached rows at once so chips, header and menus follow live; the
      // debounced refetch settles the rest, and the session's orphans are re-asked.
      on('SessionOwnerChanged', (e) => {
        qc.setQueryData<SessionSummary[]>(queryKeys.sessions, (rows) => withOwner(rows, e))
        void qc.invalidateQueries({ queryKey: queryKeys.sessionOrphans(e.session) })
        void qc.invalidateQueries({ queryKey: queryKeys.sessionsStatus })
        sessions()
      }),
      on('McpProgress', () => void qc.invalidateQueries({ queryKey: ['mcp'] })),
      on('LoginFinished', () => {
        void qc.invalidateQueries({ queryKey: queryKeys.logins })
        models()
      }),
      on('KnowledgeChanged', (e) => {
        void qc.invalidateQueries({ queryKey: queryKeys.knowledgeList })
        void qc.invalidateQueries({ queryKey: queryKeys.knowledgeDoc(e.id) })
        void qc.invalidateQueries({ queryKey: queryKeys.knowledgeHistories })
        search()
      }),
      on('ArtifactShown', () => {
        void qc.invalidateQueries({ queryKey: queryKeys.artifacts })
        search()
      }),
      on('DocumentChanged', () => void qc.invalidateQueries({ queryKey: queryKeys.notes })),
      on('SecretsChanged', () => void qc.invalidateQueries({ queryKey: queryKeys.secrets })),
      on('SecretResolved', () => void qc.invalidateQueries({ queryKey: queryKeys.secrets })),
      on('CommunicationChanged', () => {
        void qc.invalidateQueries({ queryKey: ['communication'] })
        void qc.invalidateQueries({ queryKey: ['peers'] })
        void qc.invalidateQueries({ queryKey: queryKeys.info })
      }),
      on('PeerMessageChanged', (e) => {
        void qc.invalidateQueries({ queryKey: ['peer-messages', e.session] })
        void qc.invalidateQueries({ queryKey: queryKeys.peerMessage(e.session, e.id) })
      }),
      on('A2aChanged', () => {
        void qc.invalidateQueries({ queryKey: queryKeys.a2a })
        void qc.invalidateQueries({ queryKey: ['communication'] })
        void qc.invalidateQueries({ queryKey: ['peers'] })
      }),
      // 125 §8.1: the event carries the whole status — written into the query, no refetch.
      on('AdvisorChanged', (e) => qc.setQueryData(queryKeys.advisorStatus(e.session), e.status)),
      // 53 (03 §3): the list and the current status follow the state machine; a title lands at `Summarized`.
      on('Started', (e) => {
        void qc.invalidateQueries({ queryKey: queryKeys.meetings })
        void qc.invalidateQueries({ queryKey: queryKeys.meetingStatus })
        void qc.invalidateQueries({ queryKey: queryKeys.meetingPermissions })
        void qc.invalidateQueries({ queryKey: queryKeys.meeting(e.meeting) })
      }),
      on('State', (e) => {
        void qc.invalidateQueries({ queryKey: queryKeys.meetings })
        void qc.invalidateQueries({ queryKey: queryKeys.meetingStatus })
        void qc.invalidateQueries({ queryKey: queryKeys.meeting(e.meeting) })
      }),
      on('Deleted', () => {
        void qc.invalidateQueries({ queryKey: queryKeys.meetings })
        void qc.invalidateQueries({ queryKey: queryKeys.meetingStatus })
      }),
      on('ReviewReady', (e) => {
        void qc.invalidateQueries({ queryKey: queryKeys.meeting(e.meeting) })
        void qc.invalidateQueries({ queryKey: queryKeys.meetingTranscript(e.meeting) })
      }),
      on('Summarized', (e) => {
        void qc.invalidateQueries({ queryKey: queryKeys.meetings })
        void qc.invalidateQueries({ queryKey: queryKeys.meeting(e.meeting) })
        void qc.invalidateQueries({ queryKey: queryKeys.meetingSummary(e.meeting) })
        void qc.invalidateQueries({ queryKey: queryKeys.meetingTranscript(e.meeting) })
      }),
      on('BriefChanged', () => void qc.invalidateQueries({ queryKey: queryKeys.briefs })),
      // 79 §4.4: VM rows are patched in place; the bring-up log per name; `OpenDesktop` (the `sandbox_desktop` tool) pops
      // the window out of the main shell only — pop-outs receive the same stream. `Changed` is also a `ScheduleEvent`.
      on('Changed', (e) => {
        if ('sandbox' in e) applySandboxEvent(qc, e)
      }),
      on('Removed', (e) => applySandboxEvent(qc, e)),
      on('Progress', (e) => applySandboxEvent(qc, e)),
      on('OpenDesktop', (e) => {
        if (!isPopout()) void openPopout({ kind: 'vm', name: e.name })
      }),
      // 67 §4, bridged since 79: the Mesh page's peers and lease follow the bus instead of the poll alone.
      on('peer', () => void qc.invalidateQueries({ queryKey: ['mesh'] })),
      on('lease', () => void qc.invalidateQueries({ queryKey: ['mesh'] })),
      // 137 §11.2: the compactor advanced (throttled server-side to 2 Hz); the ∞ badge and Memory panel re-read the view.
      on('EndlessProgress', (e) => void qc.invalidateQueries({ queryKey: queryKeys.sessionMemory(e.session) })),
    ]
    return () => {
      clearTimeout(timer)
      window.removeEventListener('theuth:reload-session', models)
      for (const off of offs) off()
    }
  }, [qc])
}
