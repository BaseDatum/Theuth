import { GitFork, Mic, Pencil, X } from 'lucide-react'
import { memo } from 'react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { type AttachRef, compose, parse } from '@/lib/attach'
import { type Edit, useDrafts } from '@/lib/drafts'
import { useApi } from '@/lib/rpc'
import { parseNote } from '@/lib/voice-note'
import type { ContextEntry } from '@/rpc/bindings/ContextEntry'
import { useSessionField } from '@/store/sessions'
import { AttachChip } from './AttachChip'
import { Attachments } from './Attachments'
import { ActionButton, RemoveButton, rewind, useFork } from './MessageAssistant'
import { SkillCard } from './SkillCard'

/** The message's references as a chip row above the bubble (66 §7.9); `onDrop` (editing) adds a `×` per chip. */
function ChipRow({ refs, session, onDrop }: { refs: AttachRef[]; session?: string; onDrop?: (i: number) => void }) {
  if (refs.length === 0) return null
  return (
    <div className="flex max-w-[85%] flex-wrap justify-end gap-1.5 md:max-w-[70%]">
      {refs.map((r, i) => (
        <span key={`${r.kind}:${r.id}`} className="relative">
          <AttachChip item={r} again={session} />
          {onDrop && (
            <button
              type="button"
              aria-label={`Remove ${r.title}`}
              onClick={() => onDrop(i)}
              className="absolute -top-1.5 -right-1.5 flex size-4 items-center justify-center rounded-full bg-foreground text-background"
            >
              <X className="size-2.5" />
            </button>
          )}
        </span>
      ))}
    </div>
  )
}

/** The bubble as a textarea: Enter sends, Shift+Enter breaks the line, Escape cancels. Its state is the store's `edit`
 * (`lib/drafts`): a virtualised row scrolling out and back keeps the edit, and the composer draft is never involved.
 * Edits the body; the marker lines are re-prepended on send, minus any chip the `×` dropped (66 §7.9). */
function EditBubble({
  session,
  edit,
  onSend,
  onCancel,
}: {
  session: string
  edit: Edit
  onSend: (text: string) => void
  onCancel: () => void
}) {
  const { text: value, refs: kept } = edit
  const patch = (p: Partial<Omit<Edit, 'turn'>>) => useDrafts.getState().updateEdit(session, p)
  const send = () => {
    const t = value.trim()
    if (t || kept.length > 0) onSend(compose(kept, t))
  }
  return (
    <>
      <ChipRow refs={kept} onDrop={(i) => patch({ refs: kept.filter((_, j) => j !== i) })} />
      <Textarea
        autoFocus
        value={value}
        onChange={(e) => patch({ text: e.target.value })}
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

/** Right-aligned bubble; a full-width `SkillCard` above the words when the input was a skill injection. `turn` is set only on the message that opened a turn (steers carry none).
 * `context` is the stored row's membership (48 §3.3): its attachments are the authoritative occurrences (`files` is our optimistic list until then), and a removed row renders as archive.
 * `readOnly` (an inspection page) shows text and attachments only: no edit, fork or removal controls.
 * Leading `[attached …]`/`[doc …]` lines (66 §7.9) render as chips; the bubble shows the body only. */
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
  const edit = useDrafts((s) => s.edits[session])
  const editing = !readOnly && turn != null && edit?.turn === turn
  // 45 §7.4 / 66 §7.9: the markers are the text, so own sends and history render alike.
  const { refs, body } = parse(text)
  const note = parseNote(body)
  return (
    <div className="group flex flex-col items-end gap-2 py-2">
      {skill && <SkillCard name={skill} />}
      <Attachments files={files} occurrences={context?.attachments} session={readOnly ? undefined : session} active={context?.active} className="justify-end" />
      {!editing && <ChipRow refs={refs} session={readOnly ? undefined : session} />}
      {note && (
        <span className="flex items-center gap-1 text-xs text-fg-secondary">
          <Mic className="size-3" /> {note.secs} s
        </span>
      )}
      {editing ? (
        <EditBubble
          session={session}
          edit={edit}
          onCancel={() => useDrafts.getState().endEdit(session)}
          onSend={(t) => {
            useDrafts.getState().endEdit(session)
            rewind(session, turn, () => api.edit_turn(session, turn, t, []))
          }}
        />
      ) : (
        body && (
          <div className="max-w-[85%] rounded-[22px] bg-layer-3 px-4 py-2.5 whitespace-pre-wrap break-words md:max-w-[70%]">
            {note ? note.body : body}
          </div>
        )
      )}
      {!readOnly && !editing && !context?.removed && (context || (turn != null && !running)) && (
        <div className="-mr-2 flex justify-end opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100 pointer-coarse:opacity-100">
          {turn != null && !running && (
            <>
              <ActionButton icon={Pencil} label="Edit" onClick={() => useDrafts.getState().beginEdit(session, { turn, text: body, refs })} />
              <ActionButton icon={GitFork} label="Fork from here" onClick={() => void fork(turn)} />
            </>
          )}
          {context && <RemoveButton session={session} target={context.target} active={context.active} />}
        </div>
      )}
    </div>
  )
})
