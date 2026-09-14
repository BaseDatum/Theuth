import { memo } from 'react'
import { Attachments } from './Attachments'

/** Right-aligned bubble; a `◇ skill` chip above the words when the input was a skill injection. */
export const MessageUser = memo(function MessageUser({
  text,
  skill,
  files,
}: {
  text: string
  skill?: string
  files?: string[]
}) {
  return (
    <div className="flex flex-col items-end gap-2 py-2">
      {skill && <span className="rounded-full bg-layer-2 px-3 py-1 text-sm text-fg-secondary">◇ {skill}</span>}
      {files && <Attachments files={files} className="justify-end" />}
      {text && (
        <div className="max-w-[70%] rounded-[22px] bg-layer-3 px-4 py-2.5 whitespace-pre-wrap break-words">{text}</div>
      )}
    </div>
  )
})
