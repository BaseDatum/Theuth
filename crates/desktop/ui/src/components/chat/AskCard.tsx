import { memo, useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from 'cn'
import { useApi } from '@/lib/rpc'
import type { Answer } from '@/rpc/bindings/Answer'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import type { Question } from '@/rpc/bindings/Question'

/** Draft answer per question: an option, typed text, an explicit skip, or nothing yet. */
type Draft = Answer | null

function toAnswer(d: Draft): Answer {
  if (!d) return { kind: 'skipped' }
  if (d.kind === 'text' && d.text.trim() === '') return { kind: 'skipped' }
  if (d.kind === 'text') return { kind: 'text', text: d.text.trim() }
  return d
}

function QuestionBlock({ q, draft, onChange }: { q: Question; draft: Draft; onChange(d: Draft): void }) {
  const typing = draft?.kind === 'text'
  const skipped = draft?.kind === 'skipped'
  return (
    <div className="flex flex-col gap-2">
      <div className="font-medium">{q.question}</div>
      <div className="flex flex-col gap-1">
        {q.options.map((c, i) => {
          const on = draft?.kind === 'choice' && draft.index === i
          return (
            <button
              key={c.label}
              type="button"
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
        <div className="flex items-center gap-2">
          <Input
            placeholder="Type your own…"
            value={typing ? draft.text : ''}
            onChange={(e) => onChange({ kind: 'text', text: e.target.value })}
            className={cn('h-8 text-sm', typing && 'border-fg-accent')}
          />
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
      </div>
    </div>
  )
}

/** The model's questions, all at once: pick, type, or skip each; Send answers or Dismiss (ends the turn). */
export const AskCard = memo(function AskCard({ ask }: { ask: AskRequested }) {
  const api = useApi()
  const [busy, setBusy] = useState(false)
  const [drafts, setDrafts] = useState<Draft[]>(() =>
    ask.questions.map((q) => (q.recommended === undefined ? null : { kind: 'choice', index: q.recommended })),
  )
  const answered = drafts.every((d) => d !== null && !(d.kind === 'text' && d.text.trim() === ''))
  const send = async (answers: Answer[] | null) => {
    setBusy(true)
    try {
      if (!(await api.answer_ask(ask.id, answers))) toast.error('the question was no longer pending')
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="my-2 flex flex-col gap-4 rounded-xl border border-info-border bg-info-bg px-4 py-3 text-sm">
      <div className="font-medium text-info">
        {ask.questions.length === 1 ? 'The agent has a question' : `The agent has ${ask.questions.length} questions`}
      </div>
      {ask.questions.map((q, i) => (
        <QuestionBlock
          key={q.id}
          q={q}
          draft={drafts[i] ?? null}
          onChange={(d) => setDrafts((ds) => ds.map((x, j) => (j === i ? d : x)))}
        />
      ))}
      <div className="flex gap-2">
        <Button size="sm" disabled={busy || !answered} onClick={() => send(drafts.map(toAnswer))}>
          Send answers
        </Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => send(null)}>
          Dismiss
        </Button>
        {!answered && <span className="self-center text-xs text-fg-tertiary">answer or skip every question</span>}
      </div>
    </div>
  )
})
