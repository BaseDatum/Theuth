import { Square, Volume2 } from 'lucide-react'
import { memo, useCallback, useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from 'cn'
import { useApi } from '@/lib/rpc'
import * as speech from '@/lib/speech'
import type { Answer } from '@/rpc/bindings/Answer'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import type { Question } from '@/rpc/bindings/Question'
import { useAsks } from '@/store/approvals'
import { ActionButton } from './MessageAssistant'

/** Draft answer per question: an option, typed text, an explicit skip, or nothing yet. */
export type Draft = Answer | null

/** The starting drafts: the recommended option pre-picked, else nothing (the attention Ask screen starts the same way, 104). */
export const draftsFor = (questions: readonly Question[]): Draft[] => questions.map((q) => (q.recommended === undefined ? null : { kind: 'choice', index: q.recommended }))

/** Every question has an option, non-blank text, or an explicit skip. */
export const allAnswered = (drafts: readonly Draft[]): boolean => drafts.every((d) => d !== null && !(d.kind === 'text' && d.text.trim() === ''))

export function toAnswer(d: Draft): Answer {
  if (!d) return { kind: 'skipped' }
  if (d.kind === 'text' && d.text.trim() === '') return { kind: 'skipped' }
  if (d.kind === 'text') return { kind: 'text', text: d.text.trim() }
  return d
}

/** The question and its options, as one utterance for read-aloud. */
function spoken(q: Question): string {
  const opts = q.options.map((c) => c.label).join('. ')
  return opts ? `${q.question} Options: ${opts}.` : q.question
}

/** Read-aloud toggle for one question; `key` identifies it in the speaking store like an assistant entry id. */
function ReadAloud({ session, entryKey, text }: { session: string; entryKey: string; text: string }) {
  const api = useApi()
  const speaking = speech.useSpeaking()
  const mine = speaking?.entry === entryKey
  const toggle = () => {
    if (speaking && mine) {
      speech.stop()
      speech.setSpeaking(null)
      void api.stop_speaking(speaking.id)
    } else {
      speech.prime()
      speech.speak(api, session, entryKey, text).catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
    }
  }
  return <ActionButton icon={mine ? Square : Volume2} label={mine ? 'Stop' : 'Read aloud'} onClick={toggle} className="size-8 shrink-0 [&_svg]:size-4" />
}

/** The `Type your own…` box; the `Input` carries the dictation mic (54). */
function OwnAnswer({ value, active, onChange }: { value: string; active: boolean; onChange: (v: string) => void }) {
  return <Input placeholder="Type your own…" value={value} onChange={(e) => onChange(e.target.value)} className={cn('h-8 min-w-0 flex-1 text-base md:text-sm', active && 'border-fg-accent')} />
}

/** One question. `locked`: the answer was sent; everything renders but nothing changes. 104: the attention Ask screen renders the same block. */
export function QuestionBlock({
  q,
  draft,
  onChange,
  session,
  entryKey,
  voice,
  locked,
}: {
  q: Question
  draft: Draft
  onChange(d: Draft): void
  session: string
  entryKey: string
  voice: boolean
  locked?: boolean
}) {
  const typing = draft?.kind === 'text'
  const skipped = draft?.kind === 'skipped'
  const setText = useCallback((text: string) => onChange({ kind: 'text', text }), [onChange])
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-start gap-1">
        <div className="min-w-0 flex-1 font-medium">{q.question}</div>
        {voice && <ReadAloud session={session} entryKey={entryKey} text={spoken(q)} />}
      </div>
      <div className="flex flex-col gap-1">
        {q.options.map((c, i) => {
          const on = draft?.kind === 'choice' && draft.index === i
          if (locked && !on) return null
          return (
            <button
              key={c.label}
              type="button"
              disabled={locked}
              onClick={() => onChange({ kind: 'choice', index: i })}
              aria-pressed={on}
              className={cn(
                'flex items-start gap-2 rounded-md border px-3 py-2 text-left text-sm transition-colors',
                on ? 'border-fg-accent bg-accent' : 'border-border-subtle hover:bg-accent/60',
              )}
            >
              <span className="mt-0.5 text-xs text-fg-tertiary">{on ? '●' : '○'}</span>
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="flex items-center gap-2">
                  {c.label}
                  {q.recommended === i && (
                    <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">
                      recommended
                    </Badge>
                  )}
                </span>
                {c.description && <span className="text-xs text-fg-secondary">{c.description}</span>}
              </span>
            </button>
          )
        })}
        {locked ? (
          (typing || skipped) && (
            <Input readOnly value={typing ? draft.text : ''} placeholder="Skipped" className={cn('h-8 text-base md:text-sm', typing && 'border-fg-accent')} />
          )
        ) : (
          <div className="flex items-center gap-2">
            <OwnAnswer value={typing ? draft.text : ''} active={typing} onChange={setText} />
            <Button
              type="button"
              size="sm"
              variant={skipped ? 'secondary' : 'ghost'}
              aria-pressed={skipped}
              onClick={() => onChange(skipped ? null : { kind: 'skipped' })}
            >
              {skipped ? 'Skipped' : 'Skip'}
            </Button>
          </div>
        )}
      </div>
    </div>
  )
}

const FRAME = 'my-2 flex flex-col gap-4 rounded-xl border border-info-border bg-info-bg px-4 py-3 text-sm'

/** The model's questions, all at once: pick, type, or skip each; Send answers or Dismiss (ends the turn).
 * After Send the card stays with the choices locked until the tool row's result replaces it (`AnsweredAskCard`). */
export const AskCard = memo(function AskCard({ ask }: { ask: AskRequested }) {
  const api = useApi()
  const voice = speech.useVoice()
  const [busy, setBusy] = useState(false)
  const [sent, setSent] = useState(false)
  const [drafts, setDrafts] = useState<Draft[]>(() => draftsFor(ask.questions))
  const answered = allAnswered(drafts)
  const send = async (answers: Answer[] | null) => {
    setBusy(true)
    try {
      if (await api.answer_ask(ask.id, answers)) {
        if (answers) setSent(true)
      } else {
        // Answered elsewhere and the `AskResolved` was missed: the tool row's result is the answered state.
        useAsks.getState().resolve(ask.id)
        toast.error('the question was already answered')
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className={FRAME}>
      <div className="font-medium text-info">
        {ask.questions.length === 1 ? 'The agent has a question' : `The agent has ${ask.questions.length} questions`}
      </div>
      {ask.questions.map((q, i) => (
        <QuestionBlock
          key={q.id}
          q={q}
          draft={sent ? toAnswer(drafts[i] ?? null) : (drafts[i] ?? null)}
          onChange={(d) => setDrafts((ds) => ds.map((x, j) => (j === i ? d : x)))}
          session={ask.session}
          entryKey={`${ask.id}/${q.id}`}
          voice={voice}
          locked={sent}
        />
      ))}
      {sent ? (
        <span className="text-xs text-fg-tertiary">answered</span>
      ) : (
        <div className="flex gap-2">
          <Button size="sm" disabled={busy || !answered} onClick={() => send(drafts.map(toAnswer))}>
            Send answers
          </Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => send(null)}>
            Dismiss
          </Button>
          {!answered && <span className="self-center text-xs text-fg-tertiary">answer or skip every question</span>}
        </div>
      )}
    </div>
  )
})

/** `ask_user`'s `{questions}` arguments, or `null` when the row is not that shape. */
export function askQuestions(args: unknown): Question[] | null {
  if (typeof args !== 'object' || args === null) return null
  const qs = (args as { questions?: unknown }).questions
  return Array.isArray(qs) && qs.every((q) => typeof q?.id === 'string' && typeof q?.question === 'string') ? (qs as Question[]) : null
}

/** The answers back out of the tool's `id: value` / `id: (skipped)` lines (`tools/src/ask.rs` `render`). */
function parseAnswers(questions: Question[], output: string): Draft[] {
  const lines = output.split('\n')
  return questions.map((q) => {
    const line = lines.find((l) => l.startsWith(`${q.id}: `))
    if (!line) return { kind: 'skipped' }
    const value = line.slice(q.id.length + 2)
    if (value === '(skipped)') return { kind: 'skipped' }
    const index = q.options.findIndex((c) => c.label === value)
    return index >= 0 ? { kind: 'choice', index } : { kind: 'text', text: value }
  })
}

/** The finished `ask_user` tool row: the questions with the sent answers locked in. */
export const AnsweredAskCard = memo(function AnsweredAskCard({ questions, output, session, callId }: { questions: Question[]; output: string; session: string; callId: string }) {
  const voice = speech.useVoice()
  const answers = parseAnswers(questions, output)
  return (
    <div className={FRAME}>
      <div className="font-medium text-info">{questions.length === 1 ? 'The agent asked' : `The agent asked ${questions.length} questions`}</div>
      {questions.map((q, i) => (
        <QuestionBlock key={q.id} q={q} draft={answers[i] ?? null} onChange={() => {}} session={session} entryKey={`${callId}/${q.id}`} voice={voice} locked />
      ))}
    </div>
  )
})
