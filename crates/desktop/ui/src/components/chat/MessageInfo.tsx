import { memo } from 'react'

/** One muted line (`model → x`, `↳ spawned name`, …). */
export const MessageInfo = memo(function MessageInfo({ text }: { text: string }) {
  return <div className="py-1 text-sm text-fg-tertiary whitespace-pre-wrap">{text}</div>
})

/** One line in the danger colour. */
export const MessageError = memo(function MessageError({ text }: { text: string }) {
  return <div className="py-1 text-sm text-danger whitespace-pre-wrap">{text}</div>
})
