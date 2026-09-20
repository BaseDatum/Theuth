// 53 §4.3 / 03 §3: `/meetings/$id` — while recording: state, elapsed, degradations, pause/resume/stop, the
// live log (`me:`/`them:`; a `final: false` utterance replaces the open row of its track, `final: true`
// commits it), a typed request box, the brief log with the island's cursor mirrored, and the session
// link. After the meeting (04 §7): the post-pipeline progress bar, then the review pane, the summarizing
// spinner, the summary — and the manifest for everything else (incl. `Failed` with *Re-run post pipeline*).

import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ArrowUp, LoaderCircle } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { MeetingHeader, duration, stateLabel } from '@/components/meeting/MeetingList'
import { Review } from '@/components/meeting/Review'
import { Summary } from '@/components/meeting/Summary'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Brief } from '@/rpc/bindings/Brief'
import type { Degradation } from '@/rpc/bindings/Degradation'
import type { PostStage } from '@/rpc/bindings/PostStage'
import type { Track } from '@/rpc/bindings/Track'
import { current, live, useMeeting } from '@/store/meeting'
import { on } from '@/store/sessions'

type LogRow = { track: Track; t0_ms: number; text: string; request: boolean; open: boolean }
const STAGES: PostStage[] = ['Transcribing', 'Diarizing', 'Aligning']

const degradationText = (d: Degradation): string =>
  typeof d === 'string' ? d : 'SttDown' in d ? `STT down (${d.SttDown})` : `${d.PermissionDenied} denied`

const hhmmss = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`
}

export function ControlCenter({ id }: { id: string }) {
  const api = useApi()
  const store = useMeeting()
  const isLive = store.meeting === id && live(store.state)
  const manifest = useQuery({ queryKey: queryKeys.meeting(id), queryFn: () => api.meeting_get(id) })
  const state = isLive ? store.state : manifest.data?.state
  const session = isLive ? store.session : manifest.data?.session
  const title = manifest.data?.title ?? (manifest.data ? `Meeting with ${manifest.data.process.name}` : 'Meeting')
  const degraded = isLive ? store.degraded : (manifest.data?.degraded ?? [])
  // The store is ahead of the manifest while the pipeline runs (`PostProgress` never touches the manifest).
  const progress = store.meeting === id ? store.progress : null
  const post = progress?.stage ?? (typeof state === 'object' ? state.Post : null)
  const retry = useAction(() => api.meeting_post_retry(id), { keys: [queryKeys.meeting(id)] })

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <MeetingHeader title={title}>
        {state && !isLive && <span className="text-[12px] text-fg-tertiary">{stateLabel(state)}</span>}
        {session && isLive && (
          <Button variant="outline" size="sm" asChild>
            <Link to="/c/$session" params={{ session }}>
              Session
            </Link>
          </Button>
        )}
      </MeetingHeader>
      {manifest.isError && <Empty>{errorText(manifest.error)}</Empty>}
      {degraded.length > 0 && (
        <div className="px-6 pb-2 text-[12px] text-fg-tertiary">{degraded.map(degradationText).join(' · ')}</div>
      )}
      {isLive ? (
        <div className="flex min-h-0 flex-1 gap-4 px-6 pb-4">
          <LiveLog id={id} />
          <BriefLog id={id} cursor={current(store)?.id ?? null} />
        </div>
      ) : (
        manifest.data && (
          <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10">
            {state === 'Review' ? (
              <Review id={id} failure={manifest.data.failure} />
            ) : state === 'Summarizing' ? (
              <div className="flex items-center gap-2 py-6 text-sm text-fg-secondary">
                <LoaderCircle className="size-4 animate-spin" /> Summarizing…
                {session && (
                  <Link to="/c/$session" params={{ session }} className="text-fg-accent hover:underline">
                    watch the session
                  </Link>
                )}
              </div>
            ) : state === 'Done' ? (
              <Summary id={id} session={session ?? null} />
            ) : (
              <>
                {post && (
                  <div className="py-4">
                    <div className="flex gap-3 text-[12px]">
                      {STAGES.map((s) => (
                        <span key={s} className={cn(s === post ? 'font-semibold text-fg' : 'text-fg-tertiary')}>
                          {s}
                        </span>
                      ))}
                      <span className="ml-auto text-fg-tertiary tabular-nums">{progress?.pct ?? 0}%</span>
                    </div>
                    <div className="mt-2 h-1.5 overflow-hidden rounded bg-accent">
                      <div className="h-full bg-fg-accent transition-[width]" style={{ width: `${progress?.pct ?? 0}%` }} />
                    </div>
                  </div>
                )}
                <Section
                  title="Recording"
                  actions={
                    state === 'Failed' &&
                    manifest.data.ended_at && (
                      <Button size="sm" variant="outline" disabled={retry.isPending} onClick={() => retry.mutate()}>
                        Re-run post pipeline
                      </Button>
                    )
                  }
                >
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
                  {manifest.data.failure && <Row label="Failure">{manifest.data.failure}</Row>}
                  {manifest.data.context && <Row label="Context">{manifest.data.context}</Row>}
                </Section>
                <BriefLog id={id} cursor={null} />
              </>
            )}
          </div>
        )
      )}
    </div>
  )
}

/** `meeting_live` on load, then `Utterance` events; partials replace their track's open row in place. */
function LiveLog({ id }: { id: string }) {
  const api = useApi()
  const [rows, setRows] = useState<LogRow[]>([])
  const [text, setText] = useState('')
  const end = useRef<HTMLDivElement>(null)
  useEffect(() => {
    void api
      .meeting_live(id)
      .then((us) => setRows(us.map((u) => ({ track: u.track, t0_ms: Number(u.t0_ms), text: u.text, request: u.request, open: false }))))
      .catch(() => undefined)
    return on('Utterance', (e) => {
      if (e.meeting !== id) return
      setRows((prev) => {
        const kept = prev.filter((r) => !(r.open && r.track === e.track))
        return [...kept, { track: e.track, t0_ms: Number(e.t0_ms), text: e.text, request: e.request, open: !e.final }]
      })
    })
  }, [api, id])
  useEffect(() => end.current?.scrollIntoView({ block: 'end' }), [rows])
  const request = useAction((t: string) => api.meeting_request(t), { onSuccess: () => setText('') })
  return (
    <div className="flex min-w-0 flex-1 flex-col rounded-xl border border-border-subtle">
      <div className="min-h-0 flex-1 overflow-y-auto p-3 font-mono text-[13px] leading-5">
        {rows.length === 0 && <span className="text-fg-tertiary">Waiting for speech…</span>}
        {rows.map((r, i) => (
          <div key={`${r.track}-${r.t0_ms}-${i}`} className={cn('flex gap-2', r.open && 'text-fg-tertiary', r.request && 'bg-fg-accent/10')}>
            <span className="shrink-0 text-fg-tertiary">{hhmmss(r.t0_ms)}</span>
            <span className={cn('shrink-0', r.track === 'Mic' ? 'text-fg-accent' : 'text-fg-secondary')}>
              {r.track === 'Mic' ? 'me:' : 'them:'}
            </span>
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

/** `meeting_briefs` plus `Brief` events, oldest first; `cursor` is where the island says the user is reading. */
function BriefLog({ id, cursor }: { id: string; cursor: string | null }) {
  const api = useApi()
  const briefs = useQuery({ queryKey: queryKeys.meetingBriefs(id), queryFn: () => api.meeting_briefs(id) })
  const [extra, setExtra] = useState<Brief[]>([])
  useEffect(
    () =>
      on('Brief', (e) => {
        if (e.meeting === id) setExtra((prev) => [...prev, e.brief])
      }),
    [id],
  )
  const seen = new Set(briefs.data?.map((b) => b.id))
  const all = [...(briefs.data ?? []), ...extra.filter((b) => !seen.has(b.id))]
  return (
    <div className="flex w-80 shrink-0 flex-col gap-2 overflow-y-auto">
      {all.length === 0 && <Empty>No briefs yet.</Empty>}
      {all.map((b) => (
        <div key={b.id} className={cn('rounded-xl border border-border-subtle p-3 text-[13px]', b.id === cursor && 'border-fg-accent')}>
          <div className="flex items-baseline justify-between gap-2">
            <span className="font-semibold">{b.title}</span>
            <span className="shrink-0 text-[11px] text-fg-tertiary">
              {b.cached !== null ? (
                // 05 §7: a cached instance links its row on the briefs tab (`?id=` outlines it).
                <Link to="/meetings/briefs" search={{ id: b.cached }} className="hover:underline">
                  {b.source === 'Cache' ? 'cached' : `${b.source} · cached`}
                </Link>
              ) : (
                b.source
              )}{' '}
              · {hhmmss(Number(b.at_ms))}
            </span>
          </div>
          <ul className="mt-1 list-disc space-y-0.5 pl-4 text-fg-secondary">
            {b.bullets.map((x) => (
              <li key={x}>{x}</li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  )
}
