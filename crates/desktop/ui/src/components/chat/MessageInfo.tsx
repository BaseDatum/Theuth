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
