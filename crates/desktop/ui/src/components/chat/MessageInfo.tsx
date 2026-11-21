import { ChevronRight } from 'lucide-react'
import { memo } from 'react'
import { Details } from './disclosure'

/** One muted line (`model → x`, `↳ spawned name`, …). 201 R3-2: with a `label` (a `<ledger>` / `<recall>` / `<tools>`
 * harness note) the line is the label and `text` folds under it, collapsed by default. */
export const MessageInfo = memo(function MessageInfo({ text, label }: { text: string; label?: string }) {
  if (label === undefined) return <div className="py-1 text-sm text-fg-tertiary whitespace-pre-wrap">{text}</div>
  return (
    <Details
      part="note"
      className="group/note py-1 text-sm text-fg-tertiary"
      triggerClassName="flex items-center gap-1"
      trigger={
        <>
          <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/note:rotate-90" aria-hidden="true" />
          <span>{label}</span>
        </>
      }
    >
      <div className="mt-1 pl-4.5 whitespace-pre-wrap">{text}</div>
    </Details>
  )
})

/** One error in the danger colour: a provider refusal, a failed send. Announced; wraps with the transcript. */
export const MessageError = memo(function MessageError({ text }: { text: string }) {
  return (
    <div role="alert" className="py-1 text-sm text-danger whitespace-pre-wrap">
      {text}
    </div>
  )
})

/** 109 §8.2: the partial an interrupted model call streamed — greyed under an `interrupted — resuming` label until the
 * regenerated reply's first delta removes it. */
export const MessageInterrupted = memo(function MessageInterrupted({ text }: { text: string }) {
  return (
    <div className="py-1 text-sm text-fg-tertiary opacity-70">
      <div className="text-xs italic">interrupted — resuming</div>
      <div className="whitespace-pre-wrap">{text}</div>
    </div>
  )
})
