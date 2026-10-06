import { memo } from 'react'

/** One muted line (`model → x`, `↳ spawned name`, …). */
export const MessageInfo = memo(function MessageInfo({ text }: { text: string }) {
  return <div className="py-1 text-sm text-fg-tertiary whitespace-pre-wrap">{text}</div>
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
