import { useQuerySupport } from '@/rpc/capabilities'
// 04 §7 / 156: the transcript pane on `/meetings/$id` in every ended state (Review, Summarizing, Done) — the
// speaker legend (rename, accept a proposed profile, enroll; Review only), then the segment list: click a
// timestamp to hear the segment (Web Audio, `lib/segment-audio.ts`), tick rows (shift for a range) and use the
// sticky bar or the row's `\u22ef` menu to enroll the speech as a new speaker or fold it into an enrolled one
// (`meeting_speaker_enroll`); in Review the same selection also relabels/merges, a word click splits and a text
// click fixes it. Every edit is one RPC whose answer replaces the cached transcript. `readOnly` (Summarizing,
// Done) keeps playback, selection and enrollment but drops the edit handlers, so edits never race the summary
// turn. "Done reviewing"/"Summarize" live in the page header (`MeetingActions`), not here. 158 §10.3: *Suggest names*
// asks `meeting_suggest_speakers` once and shows one row per label (name, confidence bar, evidence); Accept renames
// and, without a linked profile, enrolls the label's speech — the engine's quality gates (§10.4) decide; a refusal
// keeps the name and says why. Nothing happens without a click (R6). 158 §13 (W6): outside macOS (no VoiceProcessingIO)
// the pane asks once per meeting "Did you use speakers (not headphones)?" → `meeting_set_speakers`, the echo gate's sample.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { LoaderCircle, Volume2 } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { getSharedAudioContext } from '@/hooks/use-audio-context'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { SegmentPlayer, playError } from '@/lib/segment-audio'
import { enrollBlock, enrolledToast, rowBlock, secondsText, selectClick, speechMs } from '@/lib/segment-select'
import { mmss } from '@/lib/time-format'
import { cn } from '@/lib/utils'
import type { EditOp } from '@/rpc/bindings/method.meeting.input'
import type { EnrollTarget } from '@/rpc/bindings/method.meeting.input'
import type { Enrolled } from "@/rpc/bindings/method.meeting.output"
import type { Transcript as MeetingTranscript } from "@/rpc/bindings/method.meeting.output"
import type { Segment } from "@/rpc/bindings/method.meeting.output"
import type { SpeakerLabel } from "@/rpc/bindings/method.meeting.output"
import type { Suggestion as SpeakerSuggestion } from "@/rpc/bindings/method.meeting.output"
import { EnrollDialog } from './EnrollDialog'
import { SegmentMenu } from './SegmentMenu'

const chipClass = (speaker: string) =>
  cn('shrink-0 rounded px-1.5 text-[11px] leading-5', speaker === 'me' ? 'bg-fg-accent/15 text-fg-accent' : 'bg-accent text-fg-secondary')

export function Review({ id, readOnly = false }: { id: string; readOnly?: boolean }) {
  const canQuery1 = useQuerySupport(["meeting_transcript"])
  const canQuery1_2 = useQuerySupport(["meeting_get"])
  const canQuery1_3 = useQuerySupport(["meeting_suggest_speakers"])
  const api = useApi()
  const qc = useQueryClient()
  const transcript = useQuery({ enabled: canQuery1, queryKey: queryKeys.meetingTranscript(id), queryFn: () => api.meeting_transcript(id) })
  const edit = useAction((ops: EditOp[]) => api.meeting_edit(id, ops), {
    onSuccess: (t) => qc.setQueryData(queryKeys.meetingTranscript(id), t),
  })
  // 158 §13: the manifest (shared with the page header) says whether the speakers question was answered.
  const askSpeakers = !isMacDesktop()
  const manifest = useQuery({ queryKey: queryKeys.meeting(id), queryFn: () => api.meeting_get(id), enabled: canQuery1_2 && (askSpeakers) })
  const speakers = useAction((used: boolean) => api.meeting_set_speakers(id, used), {
    onSuccess: (m) => qc.setQueryData(queryKeys.meeting(id), m),
  })
  // Cached, never fetched on its own: the button refetches; Accept/Not them drop rows from the cached list.
  const suggestions = useQuery({ queryKey: queryKeys.meetingSuggestions(id), queryFn: () => api.meeting_suggest_speakers(id), enabled: canQuery1_3 && (false), staleTime: Infinity })
  const suggest = () =>
    void suggestions.refetch().then((r) => {
      if (r.error) toast.error(errorText(r.error))
      else if (r.data?.length === 0) toast.message('No name suggestions for this meeting')
    })
  const hide = (label: string) => qc.setQueryData(queryKeys.meetingSuggestions(id), (list?: SpeakerSuggestion[]) => list?.filter((s) => s.label !== label))
  const accept = useAction(
    async (s: SpeakerSuggestion) => {
      const t = await api.meeting_edit(id, [{ op: 'Rename', speaker: s.label, name: s.name }])
      qc.setQueryData(queryKeys.meetingTranscript(id), t)
      hide(s.label)
      // A voiceprint match is linked by the rename itself; a Warning carries no voice to enroll.
      if (s.profile || s.source === 'Warning') return
      // 158 §10.4: the automatic path runs the engine's quality gates; a refusal keeps the name.
      const target: EnrollTarget = { kind: 'New', name: s.name, entity: null, gated: true }
      try {
        // Every segment of the label, one track by construction; the engine keeps the first ≤ 30 s (`ENROLL_MAX_MS`).
        const r = await api.meeting_speaker_enroll(id, t.segments.filter((g) => g.speaker === s.label).map((g) => g.id), target)
        qc.setQueryData(queryKeys.meetingTranscript(id), r.transcript)
        void qc.invalidateQueries({ queryKey: queryKeys.meetingSpeakers })
        toast.success(enrolledToast(r, target))
      } catch (e) {
        toast.message(`${s.name} named; voice not enrolled`, { description: errorText(e) })
      }
    },
  )
  const t = transcript.data
  const names = new Map(t?.speakers.map((s) => [s.id, s.name]))
  const rows = suggestions.data?.filter((s) => s.label !== 'me' && names.has(s.label) && names.get(s.label) !== s.name) ?? []
  return (
    <div className="flex flex-col gap-4 py-2">
      {transcript.isError && <Empty>{errorText(transcript.error)}</Empty>}
      {t && (
        <>
          {!t.diarized && t.note && <p className="text-[13px] text-fg-secondary">{t.note}</p>}
          {askSpeakers && manifest.data && manifest.data.stats?.speakers == null && (
            <div className="flex flex-wrap items-center gap-2 text-[13px] text-fg-secondary">
              <span>Did you use speakers (not headphones) in this meeting?</span>
              <Button size="sm" variant="outline" disabled={speakers.isPending} onClick={() => speakers.mutate(true)}>
                Speakers
              </Button>
              <Button size="sm" variant="outline" disabled={speakers.isPending} onClick={() => speakers.mutate(false)}>
                Headphones
              </Button>
            </div>
          )}
          {!readOnly && (
            <div className="flex flex-wrap items-start gap-2">
              {t.speakers.map((s) => (
                <SpeakerChip key={s.id} label={s} onEdit={(op) => edit.mutate([op])} />
              ))}
              <Button size="sm" variant="outline" className="self-center" disabled={suggestions.isFetching} onClick={suggest}>
                {suggestions.isFetching && <LoaderCircle className="animate-spin" />} Suggest names
              </Button>
            </div>
          )}
          {!readOnly && rows.length > 0 && (
            <div className="flex flex-col gap-2">
              {rows.map((s) => (
                <SuggestionRow key={s.label} suggestion={s} speaker={names.get(s.label)!} busy={accept.isPending} onAccept={() => accept.mutate(s)} onDismiss={() => hide(s.label)} />
              ))}
              <p className="text-[12px] text-fg-tertiary">
                Suggested from calendar attendees, transcript cues and voiceprints. Nothing changes until you accept; accepting can enroll the voice when the sample passes the quality gates.
              </p>
            </div>
          )}
          <SegmentList id={id} transcript={t} onEdit={readOnly ? undefined : (ops) => edit.mutate(ops)} />
        </>
      )}
    </div>
  )
}

/** §10.3: `Speaker 2 → Priya Shah · 0.84`, the confidence bar, the evidence line, Accept / Not them. */
function SuggestionRow({ suggestion: s, speaker, busy, onAccept, onDismiss }: { suggestion: SpeakerSuggestion; speaker: string; busy: boolean; onAccept: () => void; onDismiss: () => void }) {
  const pct = Math.round(Math.min(1, Math.max(0, s.confidence)) * 100)
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-2 text-[13px]">
      <div className="flex items-center gap-2">
        <span className={chipClass(s.label)}>{speaker}</span>
        <span className="text-fg-tertiary">{'\u2192'}</span>
        <b className="font-medium">{s.name}</b>
        <span className="ml-auto tabular-nums text-fg-tertiary">{(pct / 100).toFixed(2)}</span>
      </div>
      <div className="h-1 overflow-hidden rounded bg-accent" role="meter" aria-label="Confidence" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <div className="h-full bg-fg-accent" style={{ width: `${pct}%` }} />
      </div>
      <div className="text-fg-secondary">{s.evidence}</div>
      <div className="flex gap-1">
        <Button size="xs" aria-label={`Accept ${s.name} for ${speaker}`} disabled={busy} onClick={onAccept}>
          Accept
        </Button>
        <Button size="xs" variant="ghost" aria-label={`Not ${s.name}`} disabled={busy} onClick={onDismiss}>
          Not them
        </Button>
      </div>
    </div>
  )
}

/** One legend entry: the name (click → rename), a proposed profile with *Accept*, and *Enroll…* for any label. */
function SpeakerChip({ label, onEdit }: { label: SpeakerLabel; onEdit: (op: EditOp) => void }) {
  const [mode, setMode] = useState<'view' | 'rename' | 'enroll'>('view')
  const [name, setName] = useState(label.name)
  const [entity, setEntity] = useState('')
  const close = () => setMode('view')
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-2 text-[13px]">
      <div className="flex items-center gap-2">
        {mode === 'rename' ? (
          <form
            className="flex gap-1"
            onSubmit={(e) => {
              e.preventDefault()
              if (name.trim() && name.trim() !== label.name) onEdit({ op: 'Rename', speaker: label.id, name: name.trim() })
              close()
            }}
          >
            <Input autoFocus className="h-7 w-40" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && close()} />
            <Button size="xs" type="submit">
              Rename
            </Button>
          </form>
        ) : (
          <button type="button" className={cn(chipClass(label.id), 'hover:underline')} title="Rename" onClick={() => setMode('rename')}>
            {label.name}
          </button>
        )}
        {label.profile && <span className="text-[11px] text-fg-tertiary">enrolled</span>}
        {mode !== 'enroll' && (
          <Button size="xs" variant="ghost" onClick={() => setMode('enroll')}>
            Enroll…
          </Button>
        )}
      </div>
      {label.proposed && !label.profile && (
        <div className="flex items-center gap-2 text-fg-secondary">
          looks like {label.proposed.name} ({Math.round(label.proposed.score * 100)}%)
          <Button size="xs" variant="outline" onClick={() => onEdit({ op: 'Enroll', speaker: label.id, name: label.proposed!.name, entity: null })}>
            Accept
          </Button>
        </div>
      )}
      {mode === 'enroll' && (
        <form
          className="flex gap-1"
          onSubmit={(e) => {
            e.preventDefault()
            if (!name.trim()) return
            onEdit({ op: 'Enroll', speaker: label.id, name: name.trim(), entity: entity.trim() || null })
            close()
          }}
        >
          <Input autoFocus className="h-7 w-36" placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && close()} />
          <Input className="h-7 w-36" placeholder="Knowledge entity (optional)" value={entity} onChange={(e) => setEntity(e.target.value)} />
          <Button size="xs" type="submit">
            Enroll
          </Button>
        </form>
      )}
    </div>
  )
}

/**
 * `[mm:ss] chip text \u22ef` per segment, in every ended state: timestamp → play/stop, checkbox (shift for a
 * range) → the sticky bar with the speech total and the enroll actions, `\u22ef` → `SegmentMenu`. With `onEdit`
 * (Review) the bar also relabels/merges, the text click → `FixText`, the words view → `Split`.
 */
export function SegmentList({ id, transcript, onEdit }: { id: string; transcript: MeetingTranscript; onEdit?: (ops: EditOp[]) => void }) {
  const canQuery2 = useQuerySupport(["meeting_speakers"])
  const api = useApi()
  const qc = useQueryClient()
  const [playing, setPlaying] = useState<number | null>(null)
  const player = useRef<SegmentPlayer>(null)
  player.current ??= new SegmentPlayer(getSharedAudioContext, setPlaying)
  useEffect(() => () => player.current?.stop(), [])
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [anchor, setAnchor] = useState<number | null>(null)
  const [editing, setEditing] = useState<number | null>(null)
  const [words, setWords] = useState<Set<number>>(new Set())
  /** Segments the enroll dialog is open for. */
  const [enrolling, setEnrolling] = useState<number[] | null>(null)
  const profiles = useQuery({ enabled: canQuery2, queryKey: queryKeys.meetingSpeakers, queryFn: () => api.meeting_speakers() })
  const names = new Map(transcript.speakers.map((s) => [s.id, s.name]))
  const order = transcript.segments.map((s) => s.id)
  const ids = order.filter((k) => selected.has(k))
  const speech = speechMs(transcript.segments, selected)
  const block = enrollBlock(transcript.segments, selected)

  const play = (seg: Segment) =>
    player.current
      ?.toggle(seg.id, Number(seg.t0_ms), Number(seg.t1_ms), (t0, t1) => api.meeting_segment_audio(id, seg.track, t0, t1))
      .catch((e: unknown) => toast.error(playError(e)))
  const enrolled = (r: Enrolled, target: EnrollTarget) => {
    qc.setQueryData(queryKeys.meetingTranscript(id), r.transcript)
    void qc.invalidateQueries({ queryKey: queryKeys.meetingSpeakers })
    toast.success(enrolledToast(r, target))
    setSelected(new Set())
  }
  const addTo = useAction(
    ({ segments, profile }: { segments: number[]; profile: string }) => api.meeting_speaker_enroll(id, segments, { kind: 'Existing', profile }),
    { onSuccess: (r, v) => enrolled(r, { kind: 'Existing', profile: v.profile }) },
  )
  /** What a row's menu acts on: the selection when the row is in a multi-selection, else the row alone. */
  const targets = (seg: Segment) => (selected.has(seg.id) && ids.length > 1 ? ids : [seg.id])
  const toggleWords = (k: number) => {
    const next = new Set(words)
    if (!next.delete(k)) next.add(k)
    setWords(next)
  }
  const commit = (seg: Segment, text: string) => {
    setEditing(null)
    if (onEdit && text.trim() && text !== seg.text) onEdit([{ op: 'FixText', segment: seg.id, text }])
  }
  const bulk = (ops: EditOp[]) => {
    onEdit?.(ops)
    setSelected(new Set())
  }
  return (
    <div className="flex flex-col gap-2">
      {ids.length > 0 && (
        <div className="sticky top-0 z-10 flex flex-wrap items-center gap-2 rounded-lg border border-border-subtle bg-background p-2 text-[13px]">
          <span className="text-fg-secondary">
            {ids.length} selected {'\u00b7'} {secondsText(speech)} of speech
          </span>
          <Button size="sm" variant="outline" disabled={block !== null} title={block ?? undefined} onClick={() => setEnrolling(ids)}>
            Enroll as new{'\u2026'}
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" variant="outline" disabled={block !== null || addTo.isPending} title={block ?? undefined}>
                Add to {'\u25b8'}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {profiles.data?.length ? (
                profiles.data.map((p) => (
                  <DropdownMenuItem key={p.id} onSelect={() => addTo.mutate({ segments: ids, profile: p.id })}>
                    {p.name}
                    <span className="ml-auto pl-3 text-[11px] text-fg-tertiary">{p.samples} samples</span>
                  </DropdownMenuItem>
                ))
              ) : (
                <DropdownMenuItem disabled>{profiles.data ? 'No enrolled speakers yet' : 'Loading speakers\u2026'}</DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
          {block && <span className="text-[12px] text-fg-tertiary">{block}</span>}
          {onEdit && (
            <>
              <Select onValueChange={(speaker) => bulk([{ op: 'Relabel', segments: ids, speaker }])}>
                <SelectTrigger size="sm" className="w-44">
                  <SelectValue placeholder={'Relabel \u2192'} />
                </SelectTrigger>
                <SelectContent>
                  {transcript.speakers.map((s) => (
                    <SelectItem key={s.id} value={s.id}>
                      {s.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button size="sm" variant="outline" disabled={ids.length < 2} onClick={() => bulk([{ op: 'Merge', segments: ids }])}>
                Merge
              </Button>
            </>
          )}
          <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
            Clear
          </Button>
        </div>
      )}
      {transcript.segments.length === 0 && <Empty>No speech was transcribed.</Empty>}
      <div className="text-[13px] leading-6">
        {transcript.segments.map((seg) => {
          const own = targets(seg)
          return (
            <div key={seg.id} className={cn('flex items-start gap-2 py-0.5', selected.has(seg.id) && 'bg-fg-accent/10')}>
              <input
                type="checkbox"
                className="mt-1.5"
                aria-label={`Select segment at ${mmss(Number(seg.t0_ms))}`}
                checked={selected.has(seg.id)}
                readOnly
                onClick={(e) => {
                  setSelected(selectClick(selected, order, anchor, seg.id, e.shiftKey))
                  setAnchor(seg.id)
                }}
              />
              <button
                type="button"
                className={cn('flex shrink-0 items-center gap-1 font-mono text-fg-tertiary tabular-nums hover:text-fg-accent', playing === seg.id && 'text-fg-accent')}
                title={playing === seg.id ? 'Stop' : 'Play'}
                onClick={() => void play(seg)}
              >
                {mmss(Number(seg.t0_ms))}
                {playing === seg.id && <Volume2 className="size-3 text-fg-accent" />}
              </button>
              <span className={chipClass(seg.speaker)}>{names.get(seg.speaker) ?? seg.speaker}</span>
              {editing === seg.id ? (
                <Input
                  autoFocus
                  className="h-6 flex-1"
                  defaultValue={seg.text}
                  onBlur={(e) => commit(seg, e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') commit(seg, e.currentTarget.value)
                    else if (e.key === 'Escape') {
                      e.currentTarget.value = seg.text
                      setEditing(null)
                    }
                  }}
                />
              ) : onEdit && words.has(seg.id) ? (
                <span className="min-w-0 flex-1">
                  {seg.words.map((w, k) => (
                    <button
                      key={`${seg.id}-${k}`}
                      type="button"
                      disabled={k === 0}
                      title={k > 0 ? 'Split here' : undefined}
                      className={cn('rounded px-0.5', k > 0 && 'hover:bg-fg-accent/20')}
                      onClick={() => onEdit([{ op: 'Split', segment: seg.id, at_word: k }])}
                    >
                      {w.text}
                    </button>
                  ))}
                </span>
              ) : onEdit ? (
                <button type="button" className="min-w-0 flex-1 text-left break-words hover:bg-accent/50" title="Edit text" onClick={() => setEditing(seg.id)}>
                  {seg.text}
                </button>
              ) : (
                <span className="min-w-0 flex-1 break-words">{seg.text}</span>
              )}
              <SegmentMenu
                count={own.length}
                block={own === ids ? block : rowBlock(seg)}
                playing={playing === seg.id}
                profiles={profiles.data}
                wordsOpen={onEdit ? words.has(seg.id) : undefined}
                onPlay={() => void play(seg)}
                onWords={onEdit && seg.words.length > 1 ? () => toggleWords(seg.id) : undefined}
                onEnrollNew={() => setEnrolling(own)}
                onAddTo={(p) => addTo.mutate({ segments: own, profile: p.id })}
              />
            </div>
          )
        })}
      </div>
      {enrolling && (
        <EnrollDialog
          open
          onOpenChange={(o) => !o && setEnrolling(null)}
          count={enrolling.length}
          speechMs={speechMs(transcript.segments, enrolling)}
          profiles={profiles.data}
          enroll={(target) => api.meeting_speaker_enroll(id, enrolling, target)}
          onDone={enrolled}
        />
      )}
    </div>
  )
}
