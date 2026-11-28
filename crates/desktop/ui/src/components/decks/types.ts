export type SectionKind = 'json_render' | 'note' | 'html' | 'openui'

export type ScriptMode = 'oneshot' | 'streaming' | 'hybrid'

export type ScriptStatus = 'pending_review' | 'approved' | 'rejected' | 'superseded'

export type RuntimeState =
  | 'stopped'
  | 'starting'
  | 'streaming'
  | 'paused'
  | 'scheduled_idle'
  | 'waiting_approval'
  | 'error'

export type RuntimeControlOp = 'start' | 'pause' | 'resume' | 'stop' | 'run_once'

export type SectionRevision = {
  rev: number
  at: number
  by: string
  summary: string
  title: string
  content: string
  archived: boolean
}

export type DeckSection = {
  id: string
  alias: string
  kind: SectionKind
  title: string
  content: string
  col_span: number
  data_key?: string | null
  archived: boolean
  rev: number
  rank: number
  created_at: number
  created_by: string
  updated_at: number
  updated_by: string
}

export type DeckFocus = {
  section: string
  by: string
  at: number
}

export type DeckScriptRef = {
  active_rev?: number | null
  pending_rev?: number | null
  mode?: string | null
  schedule?: string | null
  updated_at: number
}

export type DeckDocSnapshot = {
  doc_id: string
  title: string
  description: string
  layout: string
  columns: number
  focus?: DeckFocus | null
  script_ref: DeckScriptRef
  sections: DeckSection[]
  archived_sections: DeckSection[]
}

export type DeclaredAction = {
  name: string
  title: string
  description: string
  schema: unknown
  mutates_state: boolean
  allow_while_paused: boolean
}

export type ScriptRevision = {
  deck_id: string
  rev: number
  code: string
  code_sha256: string
  /** Canonical SHA-256 revision digest over (code, declared_actions, mode, interval_secs, schedule_cron). */
  revision_sha256: string
  declared_actions: DeclaredAction[]
  mode: ScriptMode
  interval_secs?: number | null
  schedule_cron?: string | null
  status: ScriptStatus
  proposed_by: string
  approved_by?: string | null
  approved_at?: string | null
  rejected_reason?: string | null
  change_summary: string
  created_at: string
}

export type DeckSnapshot = {
  snapshot_id: string
  deck_id: string
  script_rev: number
  run_epoch: number
  seq: number
  data: unknown
  section?: string | null
  created_at: string
}

export type LiveFrame = {
  deck_id: string
  script_rev: number
  run_epoch: number
  seq: number
  data: unknown
  section?: string | null
  emitted_at: string
}

export type ToolRisk = 'read_only' | 'mutating' | 'destructive'

export type PendingApproval = {
  approval_id: string
  session_id: string
  tool: string
  risk: ToolRisk
  args: unknown
}

export type DeckRuntimeStatus = {
  deck_id: string
  session_id: string
  state: RuntimeState
  run_epoch: number
  active_rev?: number | null
  pending_rev?: number | null
  stale: boolean
  last_ok_snapshot_id?: string | null
  last_ok_at?: string | null
  last_error?: string | null
  last_error_at?: string | null
  consecutive_failures: number
  step_count: number
  checkout_alive: boolean
  pending_approval?: PendingApproval | null
}

export type InteractErrorCode =
  | 'no_approved_script'
  | 'revision_mismatch'
  | 'undeclared_action'
  | 'invalid_args'
  | 'busy'
  | 'timed_out'
  | 'waiting_approval'
  | 'paused'
  | 'execution_error'
  | 'conflict'

export type InteractResponse = {
  request_id: string
  action: string
  ok: boolean
  result?: unknown
  error?: string | null
  error_code?: InteractErrorCode | null
  private: boolean
  cached: boolean
  run_epoch: number
  script_rev: number
}

/** Build `/decks/<encodedId>` while preserving optional `?section` and `?snapshot` query params. */
export function deckRoutePath(deckId: string, section?: string | null, snapshotId?: string | null): string {
  const base = `/decks/${encodeURIComponent(deckId)}`
  const params = new URLSearchParams()
  if (section) params.set('section', section)
  if (snapshotId) params.set('snapshot', snapshotId)
  const qs = params.toString()
  return qs ? `${base}?${qs}` : base
}

/** Safely convert an epoch-ms timestamp to ISO-8601; returns `null` for non-finite or out-of-range values instead of throwing `RangeError`. */
export function safeIsoTimestamp(ms: unknown): string | null {
  if (ms === null || ms === undefined || ms === '') return null
  const n = typeof ms === 'number' ? ms : Number(ms)
  if (!Number.isFinite(n)) return null
  const d = new Date(n)
  if (Number.isNaN(d.getTime())) return null
  try {
    return d.toISOString()
  } catch {
    return null
  }
}

export type DeckSummary = {
  id: string
  title: string
  description: string
  section_count: number
  session_id: string
  state: RuntimeState
  stale: boolean
  active_rev?: number | null
  pending_rev?: number | null
  updated_ms: number
}

export type DeckDetail = {
  doc: DeckDocSnapshot
  runtime: DeckRuntimeStatus
  active_script?: ScriptRevision | null
  pending_script?: ScriptRevision | null
  last_snapshot?: DeckSnapshot | null
  live_frame?: LiveFrame | null
}

export type DeckRuntimeEvent =
  | { type: 'sync'; status: DeckRuntimeStatus; last_snapshot?: DeckSnapshot | null; live_frame?: LiveFrame | null }
  | { type: 'live_frame'; frame: LiveFrame }
  | { type: 'snapshot'; snapshot: DeckSnapshot }
  | { type: 'status'; status: DeckRuntimeStatus }
  | { type: 'approval_pending'; approval: PendingApproval }
  | { type: 'approval_cleared'; approval_id: string; approved: boolean }
  | { type: 'closed'; reason: string }

export type DeckControlAvailability = {
  start: boolean
  pause: boolean
  resume: boolean
  stop: boolean
  runOnce: boolean
}

/** Shared runtime-control button enablement across `DeckView` chrome and `StreamControls` widgets. */
export function deckControlAvailability(
  state: RuntimeState,
  hasApprovedScript: boolean,
): DeckControlAvailability {
  if (!hasApprovedScript) {
    return { start: false, pause: false, resume: false, stop: false, runOnce: false }
  }
  return {
    start: state === 'stopped' || state === 'error' || state === 'scheduled_idle',
    pause: state === 'streaming',
    resume: state === 'paused',
    stop:
      state === 'starting' ||
      state === 'streaming' ||
      state === 'paused' ||
      state === 'scheduled_idle' ||
      state === 'waiting_approval',
    runOnce:
      state === 'stopped' ||
      state === 'streaming' ||
      state === 'paused' ||
      state === 'scheduled_idle' ||
      state === 'error',
  }
}
