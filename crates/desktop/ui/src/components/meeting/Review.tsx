// 04 §7: the review pane on `/meetings/$id` once the post pipeline lands on `Review` — the speaker legend
// (rename, accept a proposed profile, enroll), the segment list (play a span, multi-select → relabel/merge,
// click a word → split, click the text → fix it), then *Done reviewing* and *Summarize*. Every edit is one
// `meeting_edit` call whose answer replaces the cached transcript. `SegmentList` doubles as the read-only
// transcript under the summary.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { LoaderCircle, Volume2 } from 'lucide-react'
import { useRef, useState } from 'react'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { EditOp } from '@/rpc/bindings/EditOp'
import type { MeetingTranscript } from '@/rpc/bindings/MeetingTranscript'
import type { Segment } from '@/rpc/bindings/Segment'
import type { SpeakerLabel } from '@/rpc/bindings/SpeakerLabel'

/** `mm:ss` on the recording clock; minutes run past 59. */
export const mmss = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}

const chipClass = (speaker: string) =>
  cn('shrink-0 rounded px-1.5 text-[11px] leading-5', speaker === 'me' ? 'bg-fg-accent/15 text-fg-accent' : 'bg-accent text-fg-secondary')

export function Review({ id, failure }: { id: string; failure: string | null }) {
  const api = useApi()
  const qc = useQueryClient()
  const transcript = useQuery({ queryKey: queryKeys.meetingTranscript(id), queryFn: () => api.meeting_transcript(id) })
  const edit = useAction((ops: EditOp[]) => api.meeting_edit(id, ops), {
    onSuccess: (t) => qc.setQueryData(queryKeys.meetingTranscript(id), t),
  })
  const summarize = useAction(() => api.meeting_summarize(id))
  const t = transcript.data
  return (
    <div className="flex flex-col gap-4 py-2">
      {failure?.startsWith('summary:') && <p className="text-sm text-danger">{failure}</p>}
      {transcript.isError && <Empty>{errorText(transcript.error)}</Empty>}
      {t && (
        <>
          {!t.diarized && t.note && <p className="text-[13px] text-fg-secondary">{t.note}</p>}
          <div className="flex flex-wrap items-start gap-2">
            {t.speakers.map((s) => (
              <SpeakerChip key={s.id} label={s} onEdit={(op) => edit.mutate([op])} />
            ))}
          </div>
          <SegmentList id={id} transcript={t} onEdit={(ops) => edit.mutate(ops)} />
          <div className="flex items-center gap-2 border-t border-border-subtle pt-4">
            {t.reviewed_at ? (
              <span className="text-[13px] text-fg-tertiary">Reviewed at {new Date(t.reviewed_at).toLocaleString()}</span>
            ) : (
              <Button variant="outline" disabled={edit.isPending} onClick={() => edit.mutate([{ op: 'Reviewed' }])}>
                Done reviewing
              </Button>
            )}
            <Button disabled={!t.reviewed_at || summarize.isPending} onClick={() => summarize.mutate()}>
              {summarize.isPending && <LoaderCircle className="animate-spin" />} Summarize
            </Button>
          </div>
        </>
      )}
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
 * `[mm:ss] chip text` per segment. With `onEdit`: checkboxes → relabel/merge toolbar, text click → `FixText`,
 * `⋯` → words, word click → `Split`. Without: read-only, playback still works.
 */
export function SegmentList({ id, transcript, onEdit }: { id: string; transcript: MeetingTranscript; onEdit?: (ops: EditOp[]) => void }) {
  const api = useApi()
  const audio = useRef<HTMLAudioElement>(null)
  const [playing, setPlaying] = useState<number | null>(null)
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [editing, setEditing] = useState<number | null>(null)
  const [words, setWords] = useState<Set<number>>(new Set())
  const names = new Map(transcript.speakers.map((s) => [s.id, s.name]))
  const play = useAction(async (seg: Segment) => {
    const b64 = await api.meeting_segment_audio(id, seg.track, Number(seg.t0_ms), Number(seg.t1_ms))
    const el = audio.current
    if (!el) return
    if (el.src) URL.revokeObjectURL(el.src)
    el.src = URL.createObjectURL(new Blob([Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))], { type: 'audio/wav' }))
    setPlaying(seg.id)
    await el.play()
  })
  const toggle = (set: Set<number>, k: number) => {
    const next = new Set(set)
    if (!next.delete(k)) next.add(k)
    return next
  }
  const commit = (seg: Segment, text: string) => {
    setEditing(null)
    if (onEdit && text.trim() && text !== seg.text) onEdit([{ op: 'FixText', segment: seg.id, text }])
  }
  const bulk = (ops: EditOp[]) => {
    onEdit?.(ops)
    setSelected(new Set())
  }
  const ids = transcript.segments.filter((s) => selected.has(s.id)).map((s) => s.id)
  return (
    <div className="flex flex-col gap-2">
      <audio ref={audio} onEnded={() => setPlaying(null)} onPause={() => setPlaying(null)} />
      {onEdit && ids.length > 0 && (
        <div className="sticky top-0 z-10 flex items-center gap-2 rounded-lg border border-border-subtle bg-background p-2 text-[13px]">
          <span className="text-fg-secondary">{ids.length} selected</span>
          <Select onValueChange={(speaker) => bulk([{ op: 'Relabel', segments: ids, speaker }])}>
            <SelectTrigger size="sm" className="w-44">
              <SelectValue placeholder="Relabel →" />
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
          <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
            Clear
          </Button>
        </div>
      )}
      {transcript.segments.length === 0 && <Empty>No speech was transcribed.</Empty>}
      <div className="text-[13px] leading-6">
        {transcript.segments.map((seg) => (
          <div key={seg.id} className={cn('flex items-start gap-2 py-0.5', selected.has(seg.id) && 'bg-fg-accent/10')}>
            {onEdit && <input type="checkbox" className="mt-1.5" checked={selected.has(seg.id)} onChange={() => setSelected(toggle(selected, seg.id))} />}
            <button
              type="button"
              className="flex shrink-0 items-center gap-1 font-mono text-fg-tertiary tabular-nums hover:text-fg-accent"
              title="Play"
              onClick={() => play.mutate(seg)}
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
            ) : words.has(seg.id) ? (
              <span className="min-w-0 flex-1">
                {seg.words.map((w, k) => (
                  <button
                    key={`${seg.id}-${k}`}
                    type="button"
                    disabled={!onEdit || k === 0}
                    title={onEdit && k > 0 ? 'Split here' : undefined}
                    className={cn('rounded px-0.5', onEdit && k > 0 && 'hover:bg-fg-accent/20')}
                    onClick={() => onEdit?.([{ op: 'Split', segment: seg.id, at_word: k }])}
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
            {seg.words.length > 1 && (
              <button
                type="button"
                className={cn('shrink-0 px-1 text-fg-tertiary hover:text-fg', words.has(seg.id) && 'text-fg-accent')}
                title="Words"
                onClick={() => setWords(toggle(words, seg.id))}
              >
                ⋯
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}
