import { Link } from '@tanstack/react-router'
import { Square, Volume2 } from 'lucide-react'
import { type Ref, memo, useCallback, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { head } from '@/components/attention/Bundle'
import { usePacket } from '@/components/attention/queries'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { cn } from 'cn'
import { useApi } from '@/lib/rpc'
import * as speech from '@/lib/speech'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import type { Answer } from '@/rpc/bindings/method.tasks.input'
import type { AskRequested } from "@/rpc/bindings/method.roster.output"
import type { Packet } from "@/rpc/bindings/method.roster.output"
import type { Question } from "@/rpc/bindings/method.roster.output"
import { useAsks } from '@/store/approvals'
import { ActionButton } from './MessageAssistant'
import { DoubleCard, DoubleChip } from './DoubleChip'
import { type Draft, allAnswered, ownPick, recommendedLabel, toAnswer, useAskDraft } from './ask-drafts'
import { usePrediction } from '@/store/double'

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

/**
 * The `Type your own…` box: a one-line `Textarea` that grows with its text (154; CSS only, `field-sizing: content` —
 * WebKit < 26.2 and Firefox keep one line, like the composer); the mic is the dictation one (54). Enter inserts a
 * newline; Send answers is the only send. `kept`: words that are not the pick stay visible but dimmed.
 */
function OwnAnswer({ value, active, kept, onChange, onFocus, ref }: { value: string; active: boolean; kept: boolean; onChange: (v: string) => void; onFocus: () => void; ref: Ref<HTMLTextAreaElement> }) {
  return (
    <Textarea
      ref={ref}
      oneLine
      rows={1}
      placeholder="Type your own…"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      onFocus={onFocus}
      className={cn('min-w-0 flex-1 pointer-coarse:mt-1.5', active && 'border-fg-accent', kept && 'text-fg-secondary')}
    />
  )
}

/**
 * One question. `locked`: the answer was sent; everything renders but nothing changes. 104: the attention Ask screen
 * renders the same block. 138: `own` is what sits in the `Type your own…` box, independent of the pick (`draft`): an
 * option or Skip changes the pick only; the words stay, dimmed, and are the pick again on focus or via the row's ○.
 * Without `own`/`onOwn` the box is the text pick itself, as before.
 */
export function QuestionBlock({
  q,
  draft,
  own,
  onChange,
  onOwn,
  session,
  entryKey,
  voice,
  locked,
}: {
  q: Question
  draft: Draft
  own?: string
  onChange(d: Draft): void
  onOwn?(text: string): void
  session: string
  entryKey: string
  voice: boolean
  locked?: boolean
}) {
  const typing = draft?.kind === 'text'
  const skipped = draft?.kind === 'skipped'
  const ownText = own ?? (typing ? draft.text : '')
  const box = useRef<HTMLTextAreaElement>(null)
  const setText = useCallback((text: string) => (onOwn ? onOwn(text) : onChange({ kind: 'text', text })), [onChange, onOwn])
  // The box's words become the pick when they are non-blank and not already chosen; ○ on a blank box just focuses it.
  const chooseOwn = (focusIfBlank: boolean) => {
    const p = ownPick(draft, ownText)
    if (p !== draft) onChange(p)
    else if (focusIfBlank && !typing) box.current?.focus()
  }
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
            // The whole answer as sent, in the box's look; an `Input` cut a long one off (154).
            <div
              className={cn(
                'min-h-8 whitespace-pre-wrap [overflow-wrap:anywhere] rounded-md border border-input bg-transparent px-3 py-[3px] text-base shadow-xs md:py-[5px] md:text-sm dark:bg-input/30',
                typing ? 'border-fg-accent' : 'text-muted-foreground',
              )}
            >
              {typing ? draft.text : 'Skipped'}
            </div>
          )
        ) : (
          // ○ and Skip stay level with the box's first line as it grows. pointer-coarse: ○ is 44 px and the one-line box 32,
          // so the box and Skip start 6 px down — where `items-center` put them.
          <div className="flex items-start gap-2">
            <Button
              type="button"
              size="icon-sm"
              variant="ghost"
              aria-label="Use your answer"
              aria-pressed={typing}
              onClick={() => chooseOwn(true)}
              className="ml-0.5 text-xs text-fg-tertiary"
            >
              {typing ? '●' : '○'}
            </Button>
            <OwnAnswer ref={box} value={ownText} active={typing} kept={!typing && ownText.trim() !== ''} onChange={setText} onFocus={() => chooseOwn(false)} />
            <Button
              type="button"
              size="sm"
              variant={skipped ? 'secondary' : 'ghost'}
              aria-pressed={skipped}
              onClick={() => onChange(skipped ? null : { kind: 'skipped' })}
              className="pointer-coarse:mt-1.5"
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

/**
 * 103 F4: the one question a decision raises carries the decision key as its id (`tasks/src/lib.rs ask_user`); with a
 * packet behind it the card is the compact relay (`RelayCard`) until expanded.
 */
function decisionKey(ask: AskRequested): string | null {
  const id = ask.questions.length === 1 ? ask.questions[0]!.id : ''
  return id.startsWith('decision:') ? id : null
}

/** The model's questions, all at once: pick, type, or skip each; Send answers or Dismiss (ends the turn).
 * After Send the card stays with the choices locked until the tool row's result replaces it (`AnsweredAskCard`). */
export const AskCard = memo(function AskCard({ ask }: { ask: AskRequested }) {
  const api = useApi()
  const voice = speech.useVoice()
  const packet = usePacket(decisionKey(ask)).data ?? null
  const [expanded, setExpanded] = useDisclosure(disclosureKey('ask', ask.id))
  const [busy, setBusy] = useState(false)
  // After Send the card keeps what went on the wire and the store entry is dropped (138).
  const [sent, setSent] = useState<Answer[] | null>(null)
  const prediction = usePrediction(ask.id)
  const { draft, pick, typeOwn, drop } = useAskDraft(ask, prediction, sent === null)
  const answered = allAnswered(draft.picks)
  const send = async (answers: Answer[] | null) => {
    setBusy(true)
    try {
      if (await api.answer_ask(ask.id, answers)) {
        if (answers) setSent(answers)
        drop()
      } else {
        // Answered elsewhere and the `AskResolved` was missed: the tool row's result is the answered state.
        useAsks.getState().resolve(ask.id)
        drop()
        toast.error('That question had already closed — your answer went to the session as a message.')
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  if (packet && !expanded && !sent) return <RelayCard packet={packet} askId={ask.id} onExpand={() => setExpanded(true)} />
  return (
    <DoubleCard ids={[ask.id]} className={FRAME}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <div className="font-medium text-info">
          {ask.questions.length === 1 ? 'The agent has a question' : `The agent has ${ask.questions.length} questions`}
        </div>
        <DoubleChip ids={[ask.id]} recommended={recommendedLabel(ask.questions)} />
      </div>
      {ask.questions.map((q, i) => (
        <QuestionBlock
          key={q.id}
          q={q}
          draft={sent ? (sent[i] ?? null) : (draft.picks[i] ?? null)}
          own={draft.own[i]}
          onChange={(d) => pick(i, d)}
          onOwn={(t) => typeOwn(i, t)}
          session={ask.session}
          entryKey={`${ask.id}/${q.id}`}
          voice={voice}
          locked={sent !== null}
        />
      ))}
      {sent ? (
        <span className="text-xs text-fg-tertiary">answered</span>
      ) : (
        <div className="flex gap-2">
          <Button size="sm" disabled={busy || !answered} onClick={() => send(draft.picks.map(toAnswer))}>
            Send answers
          </Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => send(null)}>
            Dismiss
          </Button>
          {!answered && <span className="self-center text-xs text-fg-tertiary">answer or skip every question</span>}
        </div>
      )}
    </DoubleCard>
  )
})

/** The compact relay: who asks · the question · `head(summary)` · the backed option · Open in Attention · expand.
 * 126: the double's chip (keyed by the ask or the packet) sits under the title; an L2 grace makes the relay the take-over target. */
function RelayCard({ packet, askId, onExpand }: { packet: Packet; askId: string; onExpand: () => void }) {
  const rec = packet.recommendation
  const index = rec?.option ?? packet.recommended
  const label = index === null ? undefined : packet.options[index]?.label
  const by = packet.by.replace(/^agent:/, '')
  const ids = [askId, packet.key, `decision:${packet.task}:${packet.key}`]
  return (
    <DoubleCard ids={ids} className={FRAME}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <div className="font-medium text-info">{by} asks for a decision</div>
        <DoubleChip ids={ids} recommended={label} />
      </div>
      {packet.question && <div className="font-medium">{packet.question}</div>}
      <div className="text-fg-secondary">{head(packet.summary)}</div>
      {(label || rec) && (
        <div className="flex flex-wrap items-center gap-2">
          {label && (
            <span className="flex items-center gap-2">
              {label}
              <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">
                recommended
              </Badge>
            </span>
          )}
          {rec && <span className="text-xs text-fg-secondary">{rec.text}</span>}
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        <Button asChild size="sm">
          <Link to="/attention/$id" params={{ id: `decision:${packet.task}:${packet.key}` }} search={true}>
            Open in Inbox
          </Link>
        </Button>
        <Button size="sm" variant="ghost" onClick={onExpand}>
          Answer here
        </Button>
      </div>
    </DoubleCard>
  )
}

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
