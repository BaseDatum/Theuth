import { GitFork, Mic, Pencil } from 'lucide-react'
import { memo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useApi } from '@/lib/rpc'
import { parseNote } from '@/lib/voice-note'
import type { ContextEntry } from '@/rpc/bindings/ContextEntry'
import { useSessionField } from '@/store/sessions'
import { Attachments } from './Attachments'
import { ActionButton, RemoveButton, rewind, useFork } from './MessageAssistant'

/** The bubble as a textarea: Enter sends, Shift+Enter breaks the line, Escape cancels. Mounted only while editing, so `text` seeds it once. */
function EditBubble({
  text,
  onSend,
  onCancel,
}: {
  text: string
  onSend: (text: string) => void
  onCancel: () => void
}) {
  const [value, setValue] = useState(text)
  const send = () => {
    const t = value.trim()
    if (t) onSend(t)
  }
  return (
    <>
      <Textarea
        autoFocus
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape') onCancel()
          else if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault()
            send()
          }
        }}
        className="max-w-[85%] min-h-0 rounded-[22px] border-0 bg-layer-3 px-4 py-2.5 text-base shadow-none whitespace-pre-wrap break-words md:max-w-[70%] md:text-base"
      />
      <div className="flex gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" onClick={send}>
          Send
        </Button>
      </div>
    </>
  )
}

/** Right-aligned bubble; a `◇ skill` chip above the words when the input was a skill injection. `turn` is set only on the message that opened a turn (steers carry none).
 * `context` is the stored row's membership (48 §3.3): its attachments are the authoritative occurrences (`files` is our optimistic list until then), and a removed row renders as archive.
 * `readOnly` (an inspection page) shows text and attachments only: no edit, fork or removal controls. */
export const MessageUser = memo(function MessageUser({
  text,
  skill,
  files,
  session,
  turn,
  context,
  readOnly,
}: {
  text: string
  skill?: string
  files?: string[]
  session: string
  turn?: number
  context?: ContextEntry
  readOnly?: boolean
}) {
  const api = useApi()
  const running = useSessionField(session, 'running')
  const fork = useFork(session)
  const [editing, setEditing] = useState(false)
  // 45 §7.4: the marker is the text, so own sends and history render alike.
  const note = parseNote(text)
  return (
    <div className="group flex flex-col items-end gap-2 py-2">
      {skill && (
        <span className="rounded-full bg-layer-2 px-3 py-1 text-sm text-fg-secondary">
          ◇ {skill}
        </span>
      )}
      <Attachments files={files} occurrences={context?.attachments} session={readOnly ? undefined : session} active={context?.active} className="justify-end" />
      {note && (
        <span className="flex items-center gap-1 text-xs text-fg-secondary">
          <Mic className="size-3" /> {note.secs} s
        </span>
      )}
      {editing && turn != null ? (
        <EditBubble
          text={text}
          onCancel={() => setEditing(false)}
          onSend={(t) => {
            setEditing(false)
            rewind(session, turn, () => api.edit_turn(session, turn, t, []))
          }}
        />
      ) : (
        text && (
          <div className="max-w-[85%] rounded-[22px] bg-layer-3 px-4 py-2.5 whitespace-pre-wrap break-words md:max-w-[70%]">
            {note ? note.body : text}
          </div>
        )
      )}
      {!readOnly && !editing && !context?.removed && (context || (turn != null && !running)) && (
        <div className="-mr-2 flex justify-end opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100 pointer-coarse:opacity-100">
          {turn != null && !running && (
            <>
              <ActionButton icon={Pencil} label="Edit" onClick={() => setEditing(true)} />
              <ActionButton icon={GitFork} label="Fork from here" onClick={() => void fork(turn)} />
            </>
          )}
          {context && <RemoveButton session={session} target={context.target} active={context.active} />}
        </div>
      )}
    </div>
  )
})
