// 53 §4.6 / decision 11 / 03 §0.4: the current meeting as the island and the control center see it —
// state machine, topic buttons, the brief stack with its cursor and `↑ N new` badge, the cues. Fed by
// `MeetingEvent`s through `on()` (installed once per webview) and rebuilt from `meeting_status` +
// `meeting_briefs` on mount/reconnect. Pure: side effects (`meeting_cursor`, window show/hide) are the
// island component's. `Level` never lands here (a ref drives the meters).

import { create } from 'zustand'
import type { Api } from '@/rpc/bindings/client'
import type { Brief } from "@/rpc/bindings/method.meeting.output"
import type { CaptureHealth } from "@/rpc/bindings/method.meeting.output"
import type { CaptureStatus } from "@/rpc/bindings/method.meeting.output"
import type { Degradation } from "@/rpc/bindings/method.meeting.output"
import type { MeetingEvent } from "@/rpc/bindings/event.meeting_2eevents.output"
import type { MeetingState } from "@/rpc/bindings/method.meeting.output"
import type { Status as MeetingStatus } from "@/rpc/bindings/method.meeting.output"
import type { OracleDecision } from "@/rpc/bindings/event.meeting_2eevents.output"
import type { Topic } from "@/rpc/bindings/method.meeting.output"
import type { TopicId } from "@/rpc/bindings/method.meeting.output"
import type { Track } from "@/rpc/bindings/method.meeting.output"
import type { PostStage } from "@/rpc/bindings/method.meeting.output"
import { on } from '@/store/sessions'

/** Oracle verdicts kept per meeting (57 §3.4 data path). */
export const ORACLE_KEEP = 50
/** Live transcript lines kept for the island's transcript dropdown (03 §2); `meeting_live` holds the full 2 000. */
export const LINES_KEEP = 200

/**
 * One live transcript row; `open` = rolling partial (02 §2), replaced in place by the track's next `Utterance`.
 * `speaker` (07 §4): the live label (`s1`…) of a system row once the diariser scored it — a second final
 * `Utterance` for the same `track`/`t0_ms` carries it and updates the row.
 */
export type Line = { track: Track; t0_ms: number; text: string; request: boolean; open: boolean; speaker: string | null }

export const toLine = (
  u: { track: Track; t0_ms: number; text: string; request: boolean; speaker?: string | null },
  open = false,
): Line => ({
  track: u.track,
  t0_ms: Number(u.t0_ms),
  text: u.text,
  request: u.request,
  open,
  speaker: u.speaker ?? null,
})

/** `me:` / `Speaker N:` / `them:` — what the row's speaker column shows (07 §4). */
export const who = (l: Line): string => {
  if (l.track === 'Mic') return 'me:'
  const n = l.speaker?.match(/^s(\d+)$/)?.[1]
  return n ? `Speaker ${n}:` : 'them:'
}

export type MeetingSlice = {
  meeting: string | null
  session: string | null
  state: MeetingState
  degraded: Degradation[]
  /** Recording clock at `clockAt`; ticking while `state === 'Recording'`. */
  elapsedMs: number
  clockAt: number
  topics: Topic[]
  /** Tapped, brief not arrived yet: greyed. */
  tapped: TopicId[]
  /** Oldest first; the cursor indexes it (`-1` = empty). */
  briefs: Brief[]
  cursor: number
  /** Auto briefs that arrived while stepped back (decision 11). */
  unseen: number
  listening: boolean
  approvalWaiting: boolean
  /** What the meeting's session is doing right now: `thinking…`, a tool name, or nothing between turns. */
  activity: string | null
  /** Post pipeline (04 §7): the stage `state` is in and its 0–100 progress; `null` outside `Post`. */
  progress: { stage: PostStage; pct: number } | null
  /** 57 §3.4: the last `ORACLE_KEEP` oracle verdicts, oldest first; an outcome update replaces its batch's row. */
  oracle: OracleDecision[]
  /** The batch the oracle is deciding right now (the island's `deciding` flash), until its verdict lands. */
  deciding: string | null
  /** The last `LINES_KEEP` live transcript rows, oldest first; at most one `open` row per track. */
  lines: Line[]
  /** 173: both capture tracks' health (`Capture` events / `meeting_status.capture`); all `Ok` outside a recording. */
  capture: CaptureStatus
  /** The tapped app's name (`Started.process`); `null` after a reload — `meeting_status` has none. */
  app: string | null
}

type MeetingStore = MeetingSlice & {
  apply(e: MeetingEvent): void
  /** Step the cursor (`+1` newer, `-1` older). */
  move(delta: number): void
  markTapped(topic: TopicId): void
  hydrate(status: MeetingStatus, briefs: Brief[], lines: Line[]): void
}

const EMPTY: MeetingSlice = {
  meeting: null,
  session: null,
  state: 'Idle',
  degraded: [],
  elapsedMs: 0,
  clockAt: 0,
  topics: [],
  tapped: [],
  briefs: [],
  cursor: -1,
  unseen: 0,
  listening: false,
  approvalWaiting: false,
  activity: null,
  progress: null,
  oracle: [],
  deciding: null,
  lines: [],
  capture: { mic: { state: 'Ok' }, system: { state: 'Ok' } },
  app: null,
}

/** 57 §3.4 / §3.5 counters: `14 skipped · 3 briefs · 2 escalated · 5 tier 2 calls · agreed 92 %` (the UI draws a Zap icon before it). */
export function oracleCounters(oracle: OracleDecision[]): { skipped: number; briefs: number; escalated: number; tier2: number; agreed: number | null } {
  let skipped = 0
  let briefs = 0
  let escalated = 0
  let tier2 = 0
  let judged = 0
  let agreed = 0
  for (const d of oracle) {
    if (d.disposition.kind === 'skip') skipped++
    else if (d.disposition.kind === 'brief') briefs++
    else escalated++
    if (d.tier2 !== null) tier2++
    if (d.outcome) {
      judged++
      if (d.outcome.agreed) agreed++
    }
  }
  return { skipped, briefs, escalated, tier2, agreed: judged > 0 ? Math.round((100 * agreed) / judged) : null }
}

/** The counters as one line; `null` when nothing ran yet. */
export function oracleSummary(oracle: OracleDecision[]): string | null {
  if (oracle.length === 0) return null
  const c = oracleCounters(oracle)
  const parts = [`${c.skipped} skipped`, `${c.briefs} brief${c.briefs === 1 ? '' : 's'}`, `${c.escalated} escalated`, `${c.tier2} tier 2 call${c.tier2 === 1 ? '' : 's'}`]
  if (c.agreed !== null) parts.push(`agreed ${c.agreed} %`)
  return parts.join(' · ')
}

/** The recording clock is on while recording (pauses are gaps, 01 §3). */
export const live = (state: MeetingState): boolean => state === 'Recording' || state === 'Paused'

/** Recording clock now. */
export function elapsed(s: Pick<MeetingSlice, 'state' | 'elapsedMs' | 'clockAt'>, now = Date.now()): number {
  return s.elapsedMs + (s.state === 'Recording' ? now - s.clockAt : 0)
}

/** `mm:ss` on the recording clock; minutes run past 59. */
export const mmss = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}

/** Local `HH:MM` of an RFC 3339 stamp. */
export const hhmm = (iso: string): string => {
  const d = new Date(iso)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** One track's line of {@link captureWarning}; `null` while it is heard. */
function trackWarning(h: CaptureHealth, source: string, device: string, permission: string): string | null {
  switch (h.state) {
    case 'Ok':
      return null
    case 'Recovering':
      return `Not hearing ${source} since ${hhmm(h.since)} — reconnecting…`
    case 'Lost':
      return h.reason === 'PermissionDenied'
        ? `Not hearing ${source} since ${hhmm(h.since)} — ${permission} permission is off (System Settings → Privacy & Security). Retrying…`
        : `Not hearing ${source} since ${hhmm(h.since)}. Still silent — check the ${device} device or ${permission} permission. Retrying…`
    case 'Degraded':
      return `${source}'s audio tap failed; recording all system audio instead`
    default:
      return null
  }
}

/**
 * 173: what the user should read about the capture — the system track first, then the mic; `null` when both
 * are heard. `app`: the tapped app's name, `null` when unknown.
 */
export function captureWarning(capture: CaptureStatus, app: string | null): string | null {
  const parts = [
    trackWarning(capture.system, app ?? 'the meeting app', 'output', 'System Audio Recording'),
    trackWarning(capture.mic, 'your microphone', 'input', 'Microphone'),
  ].filter((p): p is string => p !== null)
  return parts.length > 0 ? parts.join(' · ') : null
}

/** 173: the strip's word for the capture health (the system track first); `null` when both tracks are heard. */
export function captureBadge(c: CaptureStatus): string | null {
  const sys = c.system
  if (sys.state === 'Lost') return sys.reason === 'PermissionDenied' ? 'no system audio permission' : 'system audio lost'
  if (sys.state === 'Recovering') return 'reconnecting audio'
  if (sys.state === 'Degraded') return 'system audio fallback'
  const mic = c.mic
  if (mic.state === 'Lost') return mic.reason === 'PermissionDenied' ? 'no mic permission' : 'mic lost'
  if (mic.state === 'Recovering') return 'reconnecting mic'
  return null
}

/** 03 §2 badges; `PermissionDenied` is the capture badge's (173 R3-6) — it says so live and clears itself. */
function badge(d: Degradation): string | null {
  if (d === 'MicNoAec') return 'headphones recommended'
  if (d === 'ModelDown') return 'model down'
  if (d === 'DiskLow') return 'disk low'
  if (d === 'NearMaxLength') return '10 min left'
  if (d === 'NoNote') return 'no note'
  if (d === 'DiarizationDown') return 'speaker labels down'
  if (typeof d === 'object' && 'SttDown' in d) return 'transcription down'
  // 173: only while it lasts; a return probe sets `until_ms`.
  if (typeof d === 'object' && 'SystemAudioFallback' in d) return d.SystemAudioFallback.until_ms === null ? 'system audio fallback' : null
  return null
}

/** The strip's degradation badges, in order, minus one that says what the capture badge (`alert`, {@link captureBadge}) already says. */
export const liveBadges = (degraded: Degradation[], alert: string | null = null): string[] =>
  degraded.map(badge).filter((b): b is string => b !== null && b !== alert)

/** Pure transition; exported for the tests. */
export function reduce(s: MeetingSlice, e: MeetingEvent): MeetingSlice {
  switch (e.type) {
    case 'Started':
      return { ...EMPTY, meeting: e.meeting, session: e.session, app: e.process.name, state: 'Recording', clockAt: Date.now() }
    case 'State': {
      if (s.meeting !== null && s.meeting !== e.meeting) return s
      const now = Date.now()
      return {
        ...s,
        meeting: e.meeting,
        state: e.state,
        degraded: e.degraded,
        elapsedMs: elapsed(s, now),
        clockAt: now,
        // The cues end with the recording.
        listening: live(e.state) && s.listening,
        approvalWaiting: live(e.state) && s.approvalWaiting,
        activity: live(e.state) ? s.activity : null,
        progress: typeof e.state === 'object' ? s.progress : null,
        capture: live(e.state) ? s.capture : EMPTY.capture,
      }
    }
    case 'TopicAdded':
      return { ...s, topics: [...s.topics.filter((t) => t.id !== e.topic.id), e.topic] }
    case 'TopicRemoved':
      return { ...s, topics: s.topics.filter((t) => t.id !== e.topic), tapped: s.tapped.filter((t) => t !== e.topic) }
    case 'Brief': {
      const briefs = [...s.briefs, e.brief]
      const top = briefs.length - 1
      // 53 §4.6: Auto at the top → show; Auto stepped back → queue and badge; anything the user asked for jumps.
      // 05 §7: a Cache brief (the actor's alias scan) is an Auto for the stack.
      const auto = e.brief.source === 'Auto' || e.brief.source === 'Cache'
      if (auto && s.briefs.length > 0 && s.cursor < s.briefs.length - 1)
        return { ...s, briefs, unseen: s.unseen + 1 }
      return { ...s, briefs, cursor: top, unseen: 0, tapped: e.brief.topic ? s.tapped.filter((t) => t !== e.brief.topic) : s.tapped }
    }
    case 'Listening':
      return { ...s, listening: e.held }
    case 'ApprovalWaiting':
      return { ...s, approvalWaiting: e.waiting }
    case 'PostProgress':
      if (s.meeting !== null && s.meeting !== e.meeting) return s
      return { ...s, meeting: e.meeting, state: { Post: e.stage }, progress: { stage: e.stage, pct: e.pct } }
    case 'ReviewReady':
      return s.meeting === e.meeting ? { ...s, state: 'Review', progress: null } : s
    case 'Deleted':
      return s.meeting === e.meeting ? EMPTY : s
    case 'Capture':
      return s.meeting === e.meeting ? { ...s, capture: e.capture } : s
    case 'OracleDeciding':
      return s.meeting === e.meeting ? { ...s, deciding: e.batch_id } : s
    case 'OracleDecision': {
      if (s.meeting !== e.meeting) return s
      const i = s.oracle.findIndex((d) => d.batch_id === e.decision.batch_id)
      const oracle = i >= 0 ? s.oracle.map((d, j) => (j === i ? e.decision : d)) : [...s.oracle, e.decision].slice(-ORACLE_KEEP)
      return { ...s, oracle, deciding: s.deciding === e.decision.batch_id ? null : s.deciding }
    }
    case 'Utterance': {
      if (s.meeting !== e.meeting) return s
      const line = toLine(e, !e.final)
      // A labelled re-publish of a committed row updates it in place (07 §3).
      const i = e.final && e.speaker ? s.lines.findIndex((l) => !l.open && l.track === e.track && l.t0_ms === line.t0_ms) : -1
      if (i >= 0) return { ...s, lines: s.lines.map((l, j) => (j === i ? line : l)) }
      const kept = s.lines.filter((l) => !(l.open && l.track === e.track))
      return { ...s, lines: [...kept, line].slice(-LINES_KEEP) }
    }
    default:
      return s
  }
}

export const useMeeting = create<MeetingStore>()((set, get) => ({
  ...EMPTY,
  apply: (e) => set(reduce(get(), e)),
  move(delta) {
    const s = get()
    const cursor = Math.min(s.briefs.length - 1, Math.max(0, s.cursor + delta))
    if (cursor === s.cursor) return
    // Stepping up eats the badge one by one; down leaves it.
    set({ cursor, unseen: Math.min(s.unseen, s.briefs.length - 1 - cursor) })
  },
  markTapped: (topic) => set((s) => ({ tapped: s.tapped.includes(topic) ? s.tapped : [...s.tapped, topic] })),
  hydrate(status, briefs, lines) {
    const at = status.cursor ? briefs.findIndex((b) => b.id === status.cursor) : -1
    set({
      ...EMPTY,
      meeting: status.meeting,
      session: status.session,
      state: status.state,
      degraded: status.degraded,
      elapsedMs: Number(status.elapsed_ms),
      clockAt: Date.now(),
      topics: status.topics,
      briefs,
      cursor: at >= 0 ? at : briefs.length - 1,
      approvalWaiting: status.approval_waiting,
      // Older/remote engines send no `capture`.
      capture: status.capture ?? EMPTY.capture,
      lines,
    })
  },
}))

/** The brief under the cursor. */
export const current = (s: Pick<MeetingSlice, 'briefs' | 'cursor'>): Brief | undefined => s.briefs[s.cursor]

/** `meeting_status` + `meeting_briefs` + `meeting_live` → the store (mount, reconnect). Errors (meetings disabled) leave it empty. */
export async function hydrateMeeting(api: Api): Promise<void> {
  const status = await api.meeting_status()
  const [briefs, lines] =
    status.meeting && live(status.state)
      ? await Promise.all([api.meeting_briefs(status.meeting), api.meeting_live(status.meeting).then((us) => us.slice(-LINES_KEEP).map((u) => toLine(u)))])
      : [[], []]
  useMeeting.getState().hydrate(status, briefs, lines)
}

/** Wire the `MeetingEvent`s (and the meeting session's turn activity) to the store; once per webview. Returns the unsubscribe. */
export function installMeetingListeners(): () => void {
  const { apply } = useMeeting.getState()
  const activity = (session: string, activity: string | null) => {
    if (useMeeting.getState().session === session) useMeeting.setState({ activity })
  }
  const offs = [
    ...(
      [
        'Started',
        'State',
        'TopicAdded',
        'TopicRemoved',
        'Brief',
        'Utterance',
        'Listening',
        'ApprovalWaiting',
        'PostProgress',
        'ReviewReady',
        'Deleted',
        'OracleDeciding',
        'OracleDecision',
        'Capture',
      ] as const
    ).map((t) =>
      on("meeting.events@1", t, apply),
    ),
    on("agent.events@1", 'TurnStarted', (e) => activity(e.session, 'thinking…')),
    on("agent.events@1", 'ToolCallStarted', (e) => activity(e.session, `${e.name}…`)),
    on("agent.events@1", 'ToolCallFinished', (e) => activity(e.session, 'thinking…')),
    on("agent.events@1", 'TurnFinished', (e) => activity(e.session, null)),
  ]
  return () => {
    for (const off of offs) off()
  }
}

