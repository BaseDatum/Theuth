import { type QueryClient, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import type { Api } from '@/rpc/api'
import type { PairCeremony } from '@/rpc/bindings/PairCeremony'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SessionMemory } from '@/rpc/bindings/SessionMemory'
import type { EndlessProgress } from '@/rpc/bindings/EndlessProgress'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'
import { isPopout, openPopout } from '@/lib/popout'
import { applySandboxEvent } from '@/store/sandboxes'
import { endlessFixture } from '@/dev/endless-fixture'
import { localFixture } from '@/dev/local-fixture'

/** Records `c` in the `meshPair` map (or a contact's in `contactPair`) under its id: a second ceremony never replaces an open one's. */
export const putPair = (qc: QueryClient, c: PairCeremony, key: readonly string[] = queryKeys.meshPair) =>
  qc.setQueryData<Record<string, PairCeremony>>(key, (m) => ({ ...m, [c.id]: c }))

/** The connected `Api`. Routes render only once connected, so a missing client is a bug. In dev the 137 and 177 fixtures may answer the endless and local-server methods (`dev/endless-fixture.ts`, `dev/local-fixture.ts`). */
export function useApi(): Api {
  const api = useConnection((s) => s.api)
  if (!api) throw new Error('useApi before the connection opened')
  return import.meta.env.DEV ? localFixture(endlessFixture(api)) : api
}

export const queryKeys = {
  sessions: ['sessions'] as const,
  /** `agents_list()` — the Agents page roster (`components/tasks/queries` `keys.agents` aliases it); refreshed with `sessions` (100 D5). */
  roster: ['roster'] as const,
  session: (id: string) => ['session', id] as const,
  /** `session_orphans(id)` (68 §4.3); dropped on `SessionOwnerChanged`. */
  sessionOrphans: (id: string) => ['session-orphans', id] as const,
  /** 183 §6.8: `mesh_nearby()`, patched in place by `MeshEvent.nearby`; under `mesh` so a peer change refetches it. */
  meshNearby: ['mesh', 'nearby'] as const,
  /** 183 §6.8: every `PairCeremony` by id, from `mesh_pair_nearby` and each `MeshEvent.pair` (no fetch of its own). */
  meshPair: ['mesh-pair'] as const,
  /** 183 §12.9: `vault_rotation()`, patched in place by `SecretsRotation`. */
  vaultRotation: ['vault-rotation'] as const,
  /** `sessions_status()` (68 §10): Settings → Network → Advanced → Sessions. */
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
  /** 177: `provider_models(name)`, the limits editor of a provider row; under `providers` so a provider change refetches it. */
  providerModels: (name: string) => ['providers', 'models', name] as const,
  /** 177: `local_servers_probe()` — fetched on Find running servers only; the rows stay through a re-probe. */
  localServers: ['local-servers'] as const,
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
  /** `subagent_model()`; Settings › Models › Defaults sets it (148 R2: native subagents' default). */
  subagentModel: ['subagent-model'] as const,
  /** `subagent_effort()` (194 row 3); the Subagents row's chips set it, a subagent model change refetches it. */
  subagentEffort: ['subagent-effort'] as const,
  /** `theuth_agent_model()` (148 R3); Settings › Models › Defaults and the Agents page's theuth tab set it. */
  theuthAgentModel: ['theuth-agent-model'] as const,
  /** `theuth_agent_effort()` (194 rows 4–5); the theuth agent pickers' chips set it, a theuth model change refetches it. */
  theuthAgentEffort: ['theuth-agent-effort'] as const,
  /** `vision_model()` (52 §3.2); slash setters and the General dropdown invalidate this same key. */
  visionModel: ['vision-model'] as const,
  /** `escalate_model()` (57 §0.9); the Agent loop dropdown and `/escalate` set it. */
  escalateModel: ['escalate-model'] as const,
  signal: ['signal'] as const,
  slack: ['slack'] as const,
  /** `slack_manifest()` (170 D1): static per engine build; not under `slack` so channel events never refetch it. */
  slackManifest: ['slack-manifest'] as const,
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
  /** 183 §12.7: `contact_list()`, under `a2a` so `A2aChanged` refetches it. */
  contacts: ['a2a', 'contacts'] as const,
  /** 183 §7.6: `a2a_exports()`, under `a2a`. */
  exports: ['a2a', 'exports'] as const,
  /** 183: `contact_first_waiting()`, under `a2a` so `A2aChanged` refetches it. */
  waiting: ['a2a', 'waiting'] as const,
  /** 183 §7.1: `contact_nearby()`, patched in place by `ContactEvent` `contact_nearby`. */
  contactNearby: ['a2a', 'nearby'] as const,
  /** 183 §7.4: every contact `PairCeremony` by id, from `contact_pair` and each `contact_pair` event (no fetch of its own). */
  contactPair: ['contact-pair'] as const,
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
  /** `meeting_templates()` (158 §11.1): Settings › Meetings' Template select and Templates list, the start form's select. */
  meetingTemplates: ['meeting-templates'] as const,
  /** `meeting_suggest_speakers(id)` (158 §10.3): fetched only when Review's *Suggest names* asks; Accept/Not them edit the cached list. */
  meetingSuggestions: (id: string) => ['meeting-suggestions', id] as const,
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

/** 183 W-7: a replicated commit is how a remote session's running state reaches this engine's `list_sessions`, so it refetches the list too. */
export function onContextChanged(search: () => void, sessions: () => void): () => void {
  return on('ContextChanged', (e) => {
    search()
    if (e.reason === 'replicated') sessions()
  })
}

/**
 * 137 §11.2: the compactor advanced (throttled server-side to 2 Hz); the ∞ badge and Memory panel re-read the view.
 * 137F: the event's `stalled` lands in the cache at once (the refetch follows), so the failing row appears and clears with it.
 */
export function applyEndlessProgress(qc: QueryClient, e: EndlessProgress): void {
  qc.setQueryData<SessionMemory>(queryKeys.sessionMemory(e.session), (m) => m && { ...m, stalled: e.stalled ?? null })
  void qc.invalidateQueries({ queryKey: queryKeys.sessionMemory(e.session) })
}

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
      void qc.invalidateQueries({ queryKey: queryKeys.theuthAgentModel })
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
      on('ObservedChildChanged', sessions),
      on('TitleChanged', () => {
        sessions()
        search()
      }),
      on('Compacted', search),
      // 183 W-7: a replicated commit is how a remote session's running state reaches this engine's `list_sessions`.
      onContextChanged(search, sessions),
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
      // 156 D4: topic briefs publish only `TopicAdded` (the model's `meeting_topic`, the alias scan), so the page's brief
      // column re-reads `meeting_briefs` on every topic/brief event rather than listening for `Brief` alone.
      on('TopicAdded', (e) => void qc.invalidateQueries({ queryKey: queryKeys.meetingBriefs(e.meeting) })),
      on('TopicRemoved', (e) => void qc.invalidateQueries({ queryKey: queryKeys.meetingBriefs(e.meeting) })),
      on('Brief', (e) => void qc.invalidateQueries({ queryKey: queryKeys.meetingBriefs(e.meeting) })),
      on('BriefChanged', () => void qc.invalidateQueries({ queryKey: queryKeys.briefs })),
      // 158 §7.6 (W2): a source was added, removed, refreshed or errored, or a push landed — Settings › Calendar's
      // rows and every Upcoming / `[▾]` list re-read.
      on('SourcesChanged', () => {
        void qc.invalidateQueries({ queryKey: ['calendar-sources'] })
        void qc.invalidateQueries({ queryKey: ['calendar-events'] })
      }),
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
      // 67 §4, bridged since 79: the Network page's peers and lease follow the bus instead of the poll alone.
      on('peer', () => void qc.invalidateQueries({ queryKey: ['mesh'] })),
      on('lease', () => void qc.invalidateQueries({ queryKey: ['mesh'] })),
      // 183 §6.8 / §12.9: Settings › Network's Nearby list, pair dialog and rotation banner carry the whole state.
      on('nearby', (e) => qc.setQueryData(queryKeys.meshNearby, e.data)),
      on('pair', (e) => {
        putPair(qc, e.data)
        if (e.data.state === 'paired') void qc.invalidateQueries({ queryKey: ['mesh'] })
      }),
      on('SecretsRotation', ({ type: _, ...state }) => qc.setQueryData(queryKeys.vaultRotation, state)),
      // 183 §7.4 / §7.1: Add contact's Nearby agents and the ceremony carry the whole state.
      on('contact_nearby', (e) => qc.setQueryData(queryKeys.contactNearby, e.data)),
      on('contact_pair', (e) => {
        putPair(qc, e.data, queryKeys.contactPair)
        if (e.data.state === 'paired') void qc.invalidateQueries({ queryKey: queryKeys.contacts })
      }),
      on('EndlessProgress', (e) => applyEndlessProgress(qc, e)),
    ]
    return () => {
      clearTimeout(timer)
      window.removeEventListener('theuth:reload-session', models)
      for (const off of offs) off()
    }
  }, [qc])
}
