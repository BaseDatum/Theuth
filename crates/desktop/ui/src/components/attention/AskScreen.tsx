import { Link } from '@tanstack/react-router'
import { QuestionBlock } from '@/components/chat/AskCard'
import { allAnswered, recommendedLabel, toAnswer, useAskDraft } from '@/components/chat/ask-drafts'
import { DoubleChip } from '@/components/chat/DoubleChip'
import { Button } from '@/components/ui/button'
import * as speech from '@/lib/speech'
import type { Answer } from '@/rpc/bindings/method.tasks.input'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import { usePrediction } from '@/store/double'
import { useQuerySupport } from '@/rpc/capabilities'
import { TONE_ACCENT, TONE_DANGER } from './actions'
import { DISMISS_HINT, dismissLabel } from './labels'
import { useAttentionBulk, useInboxActions } from './queries'
import { Body, useResolved } from './shared'
import { ActionBar, ContextLine, SnoozeButton, Source } from './source'

/**
 * 104 A6: an `ask_user` from any session — Source · the context line · `AskCard`'s question blocks with the recommended
 * option pre-picked · **Send** (`answer_ask(id, answers)`) / Snooze in the fixed bar. 207 §5.2 / review 6: the second
 * button never ends the question: with `attention_dismiss_many` it is **Leave to agent** (the agent decides, its turn
 * goes on); an older engine gets a hide-only **Dismiss** (`attention_dismiss`), never `answer_ask(id, null)`, which
 * cancels the turn. Without the questions on the item (an older engine) the screen points at the session.
 */
export function AskScreen({ item }: { item: AttentionItem }) {
  const ask = item.ask
  const act = useInboxActions()
  const bulk = useAttentionBulk()
  const leaves = useQuerySupport(['attention_dismiss_many'])
  const onResolved = useResolved(item)
  const voice = speech.useVoice()
  // 126 §9.2: the double's L1 pick pre-selects; the model's `recommended` badge stays, so a disagreement shows both.
  // 138: the draft is the chat card's too (`ask-drafts`), so it survives switching surfaces and remounts.
  const prediction = usePrediction(ask?.id)
  const { draft, pick, typeOwn, drop } = useAskDraft(ask, prediction)
  const answered = allAnswered(draft.picks)
  const busy = act.ask.isPending || bulk.pending
  const send = (answers: Answer[]) => {
    if (!ask) return
    void act.ask.mutateAsync({ id: ask.id, answers }).then(() => {
      drop()
      onResolved()
    }, () => undefined)
  }
  const leave = () => {
    void bulk.dismiss([item.id]).then((outs) => {
      if (!outs.some((o) => o.resolution !== 'failed')) return
      drop()
      onResolved()
    })
  }
  return (
    <>
      <Body>
        <Source item={item} />
        <ContextLine session={item.session} beforeMs={item.at_ms} />
        {ask ? (
          <>
            <h1 className="text-xl font-semibold leading-snug md:text-2xl">{ask.questions.length === 1 ? 'The agent has a question' : `The agent has ${ask.questions.length} questions`}</h1>
            <DoubleChip ids={[ask.id]} recommended={recommendedLabel(ask.questions)} className="w-fit" />
            <div className="flex flex-col gap-5 text-sm">
              {ask.questions.map((q, i) => (
                <QuestionBlock
                  key={q.id}
                  q={q}
                  draft={draft.picks[i] ?? null}
                  own={draft.own[i]}
                  onChange={(d) => pick(i, d)}
                  onOwn={(t) => typeOwn(i, t)}
                  session={ask.session}
                  entryKey={`${ask.id}/${q.id}`}
                  voice={voice}
                />
              ))}
            </div>
          </>
        ) : (
          <>
            <h1 className="text-xl font-semibold leading-snug md:text-2xl">{item.title}</h1>
            <p className="text-sm text-fg-secondary">
              The questions are not on this item (an older engine). Answer them in the session
              {item.session && (
                <>
                  {' — '}
                  <Link to="/c/$session" params={{ session: item.session }} className="underline underline-offset-2">
                    open it
                  </Link>
                </>
              )}
              .
            </p>
          </>
        )}
      </Body>
      <ActionBar>
        <Button size="sm" className={TONE_ACCENT} disabled={busy || !ask || !answered} onClick={() => send(draft.picks.map(toAnswer))}>
          Send
        </Button>
        {ask && !answered && <span className="text-xs text-fg-tertiary">answer or skip every question</span>}
        <span className="ml-auto flex items-center gap-1.5">
          <SnoozeButton item={item} onResolved={onResolved} />
          <Button size="sm" variant="outline" className={TONE_DANGER} disabled={busy} title={leaves ? DISMISS_HINT : 'Hide this question; the agent keeps waiting for it'} onClick={leave}>
            {dismissLabel(item, leaves)}
          </Button>
        </span>
      </ActionBar>
    </>
  )
}
