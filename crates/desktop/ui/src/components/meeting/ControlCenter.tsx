// 53 §4.3 / 03 §3: `/meetings/$id` — while recording: state, elapsed, degradations, pause/resume/stop, the
// live log (`me:`/`them:`; a `final: false` utterance replaces the open row of its track, `final: true`
// commits it), a typed request box, the brief column with the island's cursor mirrored, the session link, and the
// meeting's note beside them (59 §7.4; one scroll under `md`). After the meeting (04 §7): one status strip at the
// top — `Transcribing → Diarizing → Aligning → Review → Summarizing → Done`, the current step highlighted, a
// spinner on a running one, the failure and the action that fixes it (*Re-run post pipeline*, *Retry
// summary*) inline — and the meeting underneath it whatever the step: the transcript once there is one
// (editable in `Review`, read-only while `Summarizing` so edits never race the summary turn, and once `Done`),
// the note beside it, `summary.md` once `Done` for a meeting without a note, and the manifest (`Recording`) while
// the pipeline runs or after it failed. 156: the row under the tabs carries the state's controls (`MeetingActions`,
// D5), the briefs are a full-height resizable column on the left in every state (`BriefColumn`, D3), and the note
// is the full `NoteView` (D1). `lifecycle` is the pure state → strip mapping.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ArrowUp, Check, ChevronRight, LoaderCircle, PanelLeft, Zap } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Divider } from '@/components/chat/Panes'
import { BriefColumn, hhmmss, useBriefs } from '@/components/meeting/BriefColumn'
import { EventSwitcher } from '@/components/meeting/EventSwitcher'
import { MeetingActions } from '@/components/meeting/MeetingActions'
import { MeetingHeader, duration, stateLabel } from '@/components/meeting/MeetingList'
import { ProjectChip } from '@/components/meeting/ProjectChip'
import { MeetingNote } from '@/components/meeting/Note'
import { Review } from '@/components/meeting/Review'
import { Summary } from '@/components/meeting/Summary'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { useIsMobile } from '@/hooks/use-mobile'
import { changeProject } from '@/lib/meeting-project'
import { MAX_BRIEF_WIDTH, MIN_BRIEF_WIDTH, noteSplitAt, useMeetingSettings } from '@/lib/meeting-settings'
import { PX_STEP, RATIO_MAX, RATIO_MIN, RATIO_STEP } from '@/lib/resize-drag'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { LiveSession } from '@/routes/c.$session'
import type { Degradation } from '@/rpc/bindings/Degradation'
import type { MeetingState } from '@/rpc/bindings/MeetingState'
import type { PostStage } from '@/rpc/bindings/PostStage'
import { type Line, current, live, oracleSummary, toLine, useMeeting, who } from '@/store/meeting'
import { on } from '@/store/sessions'

/** The post-meeting lifecycle, in order; `Done` is the last step. */
export const STEPS = ['Transcribing', 'Diarizing', 'Aligning', 'Review', 'Summarizing', 'Done'] as const
export type Step = (typeof STEPS)[number]

export type Lifecycle = {
  /** Index into `STEPS` of the current step; `null` when no step is current (`Failed`). */
  current: number | null
  /** The current step is running (a spinner): a pipeline stage or `Summarizing`, never `Review`/`Done`. */
  busy: boolean
  /** Shown in the strip. */
  failure: string | null
  /** The action that fixes `failure`. */
  action: 'post_retry' | 'summarize' | null
}

/**
 * The strip for a finished recording: `null` while the meeting is live or its recording never ended (a
 * `Failed` start has no lifecycle). `post` is the running stage when the store is ahead of the manifest.
 * A `Review` whose `failure` is a summary failure offers *Retry summary*; `Failed` offers *Re-run post pipeline*.
 */
export function lifecycle(state: MeetingState | undefined, post: PostStage | null, failure: string | null, ended: boolean): Lifecycle | null {
  if (state === undefined || !ended || live(state)) return null
  const stage = post ?? (typeof state === 'object' ? state.Post : null)
  if (stage) return { current: STEPS.indexOf(stage), busy: true, failure: null, action: null }
  switch (state) {
    case 'Review':
      return { current: STEPS.indexOf('Review'), busy: false, failure, action: failure ? 'summarize' : null }
    case 'Summarizing':
      return { current: STEPS.indexOf('Summarizing'), busy: true, failure: null, action: null }
    case 'Done':
      return { current: STEPS.indexOf('Done'), busy: false, failure: null, action: null }
    case 'Failed':
      return { current: null, busy: false, failure, action: 'post_retry' }
    default:
      return null
  }
}

/** Whether the transcript of record exists: `Review` and after. */
const hasTranscript = (l: Lifecycle | null): boolean => l !== null && l.current !== null && l.current >= STEPS.indexOf('Review')

const degradationText = (d: Degradation): string =>
  d === 'NoNote'
    ? 'no meeting note: the documents helper is missing (Settings › Node › Documents)'
    : d === 'DiarizationDown'
      ? 'live speaker labels down (the post pass still labels speakers)'
      : typeof d === 'string'
        ? d
        : 'SttDown' in d
          ? `STT down (${d.SttDown})`
          : `${d.PermissionDenied} denied`

export function ControlCenter({ id }: { id: string }) {
  const api = useApi()
  const qc = useQueryClient()
  const store = useMeeting()
  const isLive = store.meeting === id && live(store.state)
  const manifest = useQuery({ queryKey: queryKeys.meeting(id), queryFn: () => api.meeting_get(id) })
  const state = isLive ? store.state : manifest.data?.state
  const session = isLive ? store.session : manifest.data?.session
  const title = manifest.data?.title ?? (manifest.data ? `Meeting with ${manifest.data.process.name}` : 'Meeting')
  const degraded = isLive ? store.degraded : (manifest.data?.degraded ?? [])
  // The store is ahead of the manifest while the pipeline runs (`PostProgress` never touches the manifest).
  const progress = store.meeting === id ? store.progress : null
  const cycle = lifecycle(state, progress?.stage ?? null, manifest.data?.failure ?? null, manifest.data?.ended_at != null)
  const retry = useAction(() => api.meeting_post_retry(id), { keys: [queryKeys.meeting(id)] })
  const summarize = useAction(() => api.meeting_summarize(id), { keys: [queryKeys.meeting(id)] })
  // 158 §9.2 (W3): the header chip; the engine re-projects the meeting's session too, so the sidebar re-reads.
  const setProject = useAction((project: string | null) => changeProject(api, qc, id, project), { keys: [queryKeys.meetings, queryKeys.sessions] })
  const note = manifest.data?.note ?? null
  const mobile = useIsMobile()
  const transcript = hasTranscript(cycle)
  // 70 POPOUT-2 item 4: centre column = the transcript area over the meeting's own chat, split by a remembered
  // divider; the note on the right, the centre/note split dragged (a ratio of the pair, like the chat panes) and
  // remembered. 156 D3: the brief column first, full height, its width dragged and remembered, collapsible from the
  // row. Narrow (under `md`, a small popped window): everything stacks in one scroll (D6), the briefs as a block
  // that is open while live and folded once the meeting ended.
  const ratio = useMeetingSettings((s) => s.split)
  const setSplit = useMeetingSettings((s) => s.setSplit)
  const briefWidth = useMeetingSettings((s) => s.briefWidth)
  const setBriefWidth = useMeetingSettings((s) => s.setBriefWidth)
  const briefsOpen = useMeetingSettings((s) => s.briefsOpen)
  const setBriefsOpen = useMeetingSettings((s) => s.setBriefsOpen)
  const noteSplit = useMeetingSettings((s) => s.noteSplit)
  const setNoteSplit = useMeetingSettings((s) => s.setNoteSplit)
  const briefs = useBriefs(id).data ?? []
  const cursor = isLive ? (current(store)?.id ?? null) : null
  const dragStart = useRef({ x: 0, width: 0 })
  const noteDrag = useRef({ ratio: 0, x: 0, pairWidth: 1 })
  const [phoneBriefs, setPhoneBriefs] = useState(isLive)
  useEffect(() => setPhoneBriefs(isLive), [isLive])
  const block = mobile ? 'h-[70dvh] flex-none' : 'min-h-0'
  // The popped window's native title follows (`on_document_title_changed`); main hides its title bar.
  useEffect(() => {
    document.title = title
  }, [title])

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <MeetingHeader title={title}>
        <EventSwitcher meeting={id} className="hidden max-w-64 md:inline-flex" />
        {manifest.data && (
          <ProjectChip value={manifest.data.project} onChange={(p) => setProject.mutate(p)} empty="No project" variant="ghost" disabled={setProject.isPending} className="hidden max-w-48 md:inline-flex" />
        )}
        {state && !isLive && <span className="shrink-0 text-[12px] text-fg-tertiary">{stateLabel(state)}</span>}
        {isLive && oracleSummary(store.oracle) && (
          <span className="hidden items-center gap-1 text-[12px] text-fg-tertiary md:flex" title="decision oracle this meeting (57 §3.4): skipped turns, briefs published, escalated, shadow agreement">
            <Zap className="size-3" />
            {oracleSummary(store.oracle)}
          </span>
        )}
        <MeetingActions id={id} state={state} cycle={cycle} compact={mobile}>
          {!mobile && (
            <Button variant="ghost" size="sm" aria-pressed={briefsOpen} aria-label={`${briefsOpen ? 'Hide' : 'Show'} briefs`} onClick={() => setBriefsOpen(!briefsOpen)}>
              <PanelLeft className={cn(!briefsOpen && 'text-fg-tertiary')} />
              Briefs · {briefs.length}
            </Button>
          )}
          {session && (
            <Button variant="outline" size={mobile ? 'xs' : 'sm'} asChild>
              <Link to="/c/$session" params={{ session }}>
                Session
              </Link>
            </Button>
          )}
        </MeetingActions>
      </MeetingHeader>
      {manifest.isError && <Empty>{errorText(manifest.error)}</Empty>}
      {degraded.length > 0 && (
        <div className="px-6 pb-2 text-[12px] text-fg-tertiary">{degraded.map(degradationText).join(' · ')}</div>
      )}
      {cycle && (
        <LifecycleStrip
          cycle={cycle}
          pct={progress?.pct ?? null}
          busy={retry.isPending || summarize.isPending}
          onAction={() => (cycle.action === 'summarize' ? summarize.mutate() : retry.mutate())}
        />
      )}
      {(isLive || manifest.data) && (
        <div className="flex min-h-0 flex-1 flex-col gap-3 px-6 pb-4">
          <div className={cn('flex min-h-0 flex-1 gap-4', mobile ? 'flex-col overflow-y-auto' : 'flex-row')}>
            {mobile ? (
              <div className="flex-none">
                <button type="button" className="flex w-full items-center gap-1 py-1 text-[13px] font-semibold" aria-expanded={phoneBriefs} onClick={() => setPhoneBriefs((o) => !o)}>
                  <ChevronRight className={cn('size-4 text-fg-tertiary transition-transform', phoneBriefs && 'rotate-90')} />
                  Briefs · {briefs.length}
                </button>
                {phoneBriefs && <BriefColumn briefs={briefs} cursor={cursor} live={isLive} className="max-h-[50dvh]" />}
              </div>
            ) : (
              briefsOpen && (
                <div className="flex min-h-0 shrink-0">
                  <BriefColumn briefs={briefs} cursor={cursor} live={isLive} style={{ width: briefWidth }} />
                  <ResizeHandle
                    orientation="vertical"
                    aria-label="Resize brief column"
                    className="ml-1 w-1"
                    value={{ now: briefWidth, min: MIN_BRIEF_WIDTH, max: MAX_BRIEF_WIDTH }}
                    onStart={(e) => (dragStart.current = { x: e.clientX, width: briefWidth })}
                    onMove={(x) => setBriefWidth(dragStart.current.width + x - dragStart.current.x)}
                    onNudge={(d) => setBriefWidth(briefWidth + d * PX_STEP)}
                  />
                </div>
              )
            )}
            <div className={mobile ? 'contents' : 'flex min-h-0 min-w-0 flex-1'}>
              {/* Alone in the wrapper (no note) the centre must grow by 1, not by its share: a grow < 1 leaves free space. `overflow-clip`, not
                  `-hidden`: at the 15 % clamp the chat composer is wider than the column, and a hidden scroll container would let a focus()
                  scroll it sideways with no way back. */}
              <div className={cn('flex min-w-0 flex-col', mobile ? 'flex-none' : 'min-h-0 overflow-clip')} style={mobile ? undefined : { flex: note ? `${noteSplit} 1 0%` : '1 1 0%' }}>
                <div className={cn('flex min-w-0 flex-col', block)} style={mobile ? undefined : { flex: `${ratio} 1 0%` }}>
                  {isLive ? (
                    <LiveLog id={id} />
                  ) : (
                    manifest.data && (
                      <div className="min-h-0 min-w-0 flex-1 overflow-y-auto pb-4">
                        {state === 'Done' && <Summary id={id} note={note} />}
                        {transcript ? (
                          <Review id={id} readOnly={state !== 'Review'} />
                        ) : (
                          <Section title="Recording">
                            <Row label="Started">{new Date(manifest.data.started_at).toLocaleString()}</Row>
                            <Row label="Ended">{manifest.data.ended_at ? new Date(manifest.data.ended_at).toLocaleString() : '—'}</Row>
                            <Row label="Duration">{manifest.data.duration_ms === null ? '—' : duration(Number(manifest.data.duration_ms))}</Row>
                            <Row label="Meeting app">
                              {manifest.data.process.name}
                              {manifest.data.process.bundle_id ? ` (${manifest.data.process.bundle_id})` : ''}
                            </Row>
                            <Row label="Consent">{new Date(manifest.data.consent_at).toLocaleString()}</Row>
                            <Row label="Pauses">
                              {manifest.data.gaps.length === 0
                                ? 'none'
                                : manifest.data.gaps.map((g) => `${hhmmss(Number(g.at_ms))} for ${Math.round(Number(g.dur_ms) / 1000)} s`).join(', ')}
                            </Row>
                            <Row label="Session model">{manifest.data.session_model ?? 'default'}</Row>
                            <Row label="Tracks">{manifest.data.tracks.map((t) => t.file).join(', ')}</Row>
                            {manifest.data.failure && !cycle?.failure && <Row label="Failure">{manifest.data.failure}</Row>}
                            {manifest.data.context && <Row label="Context">{manifest.data.context}</Row>}
                          </Section>
                        )}
                      </div>
                    )
                  )}
                </div>
                {session && (
                  <>
                    {!mobile && <Divider dir="col" ratio={ratio} onRatio={setSplit} />}
                    <div
                      className={cn('flex min-w-0 flex-col rounded-xl border border-border-subtle', block, mobile && 'mt-4')}
                      style={mobile ? undefined : { flex: `${1 - ratio} 1 0%` }}
                    >
                      <LiveSession session={session} first={false} />
                    </div>
                  </>
                )}
              </div>
              {note && !mobile && (
                <ResizeHandle
                  orientation="vertical"
                  aria-label="Resize note column"
                  className="mx-1.5 w-1"
                  value={{ now: Math.round(noteSplit * 100), min: RATIO_MIN * 100, max: RATIO_MAX * 100 }}
                  onStart={(e, el) => {
                    // The pair = the centre column + the note column (the handle's siblings), without the handle itself.
                    const pairWidth = (el.previousElementSibling?.getBoundingClientRect().width ?? 0) + (el.nextElementSibling?.getBoundingClientRect().width ?? 0)
                    noteDrag.current = { ratio: noteSplit, x: e.clientX, pairWidth }
                  }}
                  onMove={(x) => setNoteSplit(noteSplitAt(noteDrag.current, x))}
                  onNudge={(d) => setNoteSplit(noteSplit + d * RATIO_STEP)}
                />
              )}
              {note && <MeetingNote doc={note} meeting={id} className={cn(block, !mobile && 'min-w-0')} style={mobile ? undefined : { flex: `${1 - noteSplit} 1 0%` }} />}
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

/** The step indicator: done steps ticked, the current one bold with a spinner when `busy`, the failure and its fix inline. */
function LifecycleStrip({ cycle, pct, busy, onAction }: { cycle: Lifecycle; pct: number | null; busy: boolean; onAction: () => void }) {
  const bar = pct !== null && cycle.busy
  return (
    <div className="mx-6 mb-3 rounded-xl border border-border-subtle px-4 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px]">
        {STEPS.map((s, i) => {
          const at = cycle.current === i
          const done = cycle.current !== null && (i < cycle.current || (at && s === 'Done'))
          return (
            <span key={s} className={cn('flex items-center gap-1', at ? 'font-semibold text-fg' : done ? 'text-fg-secondary' : 'text-fg-tertiary')}>
              {at && cycle.busy ? <LoaderCircle className="size-3 animate-spin" /> : done ? <Check className="size-3" /> : null}
              {s}
            </span>
          )
        })}
        {bar && <span className="ml-auto text-fg-tertiary tabular-nums">{pct}%</span>}
      </div>
      {bar && (
        <div className="mt-2 h-1.5 overflow-hidden rounded bg-accent">
          <div className="h-full bg-fg-accent transition-[width]" style={{ width: `${pct}%` }} />
        </div>
      )}
      {cycle.failure && (
        <div className="mt-2 flex flex-wrap items-center gap-2 text-[13px]">
          <span className="text-danger">{cycle.failure}</span>
          {cycle.action && (
            <Button size="sm" variant="outline" disabled={busy} onClick={onAction}>
              {cycle.action === 'summarize' ? 'Retry summary' : 'Re-run post pipeline'}
            </Button>
          )}
        </div>
      )}
    </div>
  )
}

/**
 * `meeting_live` on load, then `Utterance` events; partials replace their track's open row in place, a labelled
 * re-publish (07 §3) updates the committed row with the same track and start.
 */
function LiveLog({ id }: { id: string }) {
  const api = useApi()
  const [rows, setRows] = useState<Line[]>([])
  const [text, setText] = useState('')
  const end = useRef<HTMLDivElement>(null)
  useEffect(() => {
    void api
      .meeting_live(id)
      .then((us) => setRows(us.map((u) => toLine(u))))
      .catch(() => undefined)
    return on('Utterance', (e) => {
      if (e.meeting !== id) return
      setRows((prev) => {
        const line = toLine(e, !e.final)
        const i = e.final && e.speaker ? prev.findIndex((r) => !r.open && r.track === e.track && r.t0_ms === line.t0_ms) : -1
        if (i >= 0) return prev.map((r, j) => (j === i ? line : r))
        const kept = prev.filter((r) => !(r.open && r.track === e.track))
        return [...kept, line]
      })
    })
  }, [api, id])
  // Braces on purpose: with the editor mounted, a scroll polyfill makes `scrollIntoView` return a Promise, which React rejects as a cleanup.
  useEffect(() => {
    end.current?.scrollIntoView({ block: 'end' })
  }, [rows])
  const request = useAction((t: string) => api.meeting_request(t), { onSuccess: () => setText('') })
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col rounded-xl border border-border-subtle">
      <div className="min-h-0 flex-1 overflow-y-auto p-3 font-mono text-[13px] leading-5">
        {rows.length === 0 && <span className="text-fg-tertiary">Waiting for speech…</span>}
        {rows.map((r, i) => (
          <div key={`${r.track}-${r.t0_ms}-${i}`} className={cn('flex gap-2', r.open && 'text-fg-tertiary', r.request && 'bg-fg-accent/10')}>
            <span className="shrink-0 text-fg-tertiary">{hhmmss(r.t0_ms)}</span>
            <span className={cn('shrink-0', r.track === 'Mic' ? 'text-fg-accent' : 'text-fg-secondary')}>{who(r)}</span>
            <span className="min-w-0 break-words">
              {r.request && <span className="mr-1 rounded bg-fg-accent px-1 text-[10px] text-primary-foreground">request</span>}
              {r.text}
            </span>
          </div>
        ))}
        <div ref={end} />
      </div>
      <form
        className="flex gap-2 border-t border-border-subtle p-2"
        onSubmit={(e) => {
          e.preventDefault()
          if (text.trim()) request.mutate(text.trim())
        }}
      >
        <Input value={text} placeholder="Ask the meeting session… (answer arrives as a brief)" onChange={(e) => setText(e.target.value)} />
        <Button size="icon-sm" aria-label="Send request" disabled={!text.trim() || request.isPending} type="submit">
          <ArrowUp />
        </Button>
      </form>
    </div>
  )
}
