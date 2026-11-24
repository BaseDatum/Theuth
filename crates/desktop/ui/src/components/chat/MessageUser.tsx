import { AudioLines, ExternalLink, Forward, GitFork, Link2, Mic, Paperclip, Pencil, X } from 'lucide-react'
import { memo, type ReactNode } from 'react'
import { ChannelIcon } from '@/components/icons/channels'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { type AttachRef, compose, parse } from '@/lib/attach'
import { type Segment, type SharedMessage, originLabel, originOf, parseMrkdwn, parseShared, shownText } from '@/lib/channel-origin'
import { parseAsk } from '@/lib/board-ask'
import { type Edit, useDrafts } from '@/lib/drafts'
import { type CellResultInput, parseSpoken } from '@/lib/injected'
import { useApi } from '@/lib/rpc'
import { parseScreenLines } from '@/lib/screen-context'
import { openExternal } from '@/lib/platform'
import { parseNote } from '@/lib/voice-note'
import { disclosureKey } from '@/lib/tool-runs'
import type { ChannelOrigin } from "@/rpc/bindings/method.agent.output"
import type { ContextEntry } from "@/rpc/bindings/method.agent.output"
import { useConnection } from '@/store/connection'
import { useSessionField } from '@/store/sessions'
import { AttachChip } from './AttachChip'
import { BoardQuoteCard } from './BoardQuoteCard'
import { Attachments } from './Attachments'
import { LongText } from './LongText'
import { ActionButton, CopyButton, RemoveButton, rewind, useFork } from './MessageAssistant'
import { ScreenContextCard } from './ScreenContextCard'
import { SkillCard } from './SkillCard'
import { SentResults } from './SentResults'
import { Stamp, useStampToggle } from './Stamp'

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

/** The user's words. */
const BUBBLE = 'max-w-[85%] rounded-[22px] bg-layer-3 px-4 py-2.5 whitespace-pre-wrap break-words md:max-w-[70%]'

/** 172 D4: the channel a message came in on, as a muted tag above the bubble. */
function OriginChip({ origin }: { origin: ChannelOrigin }) {
  return (
    <span className="inline-flex max-w-[85%] items-center gap-1.5 rounded-full bg-layer-2 px-2 text-xs leading-5 text-fg-secondary md:max-w-[70%]" data-channel={origin.via}>
      <ChannelIcon via={origin.via} className="size-3.5 shrink-0" />
      <span className="truncate">{originLabel(origin)}</span>
    </span>
  )
}

/** A link out of the app (`openExternal`); the click never reaches the bubble's stamp toggle. */
function OutLink({ href, className, children }: { href: string; className: string; children: ReactNode }) {
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        openExternal(href)
      }}
      className={className}
    >
      {children}
    </a>
  )
}

/** 172 D5: a forwarded or linked Slack message as a quote card; its `(channel …, ts …)` line is not shown. */
function SharedCard({ message: m }: { message: SharedMessage }) {
  const Icon = m.kind === 'forwarded' ? Forward : Link2
  return (
    <div className="my-1 w-full max-w-[85%] min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm md:max-w-[70%]" data-slack-shared={m.kind}>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-fg-secondary">
        <Icon className="size-3.5 shrink-0" aria-hidden="true" />
        <span>{m.kind === 'forwarded' ? 'Forwarded message' : 'Linked message'}</span>
        <span className="font-medium text-foreground wrap-anywhere">{m.author}</span>
        {m.channel && <span className="wrap-anywhere">#{m.channel}</span>}
        {m.time && <span>{m.time}</span>}
        {m.permalink && (
          <OutLink href={m.permalink} className="inline-flex items-center gap-1 underline-offset-4 hover:text-foreground hover:underline">
            Open in Slack <ExternalLink className="size-3" aria-hidden="true" />
          </OutLink>
        )}
      </div>
      {m.body && (
        <div className="mt-2 border-l-2 border-fg-accent pl-3 leading-6 whitespace-pre-wrap wrap-anywhere">
          {parseMrkdwn(m.body).map((p, i) =>
            p.kind === 'text' ? (
              p.text
            ) : (
              <OutLink key={i} href={p.href} className="underline underline-offset-2 hover:text-fg-accent">
                {p.text}
              </OutLink>
            ),
          )}
        </div>
      )}
      {m.files && <div className="mt-1.5 text-xs text-fg-secondary wrap-anywhere">files: {m.files}</div>}
    </div>
  )
}

/** One segment of a Slack message body: the user's words as a bubble, a shared message as a card, an attachment as one muted line. */
function SlackSegment({ segment: s, disclosure }: { segment: Segment; disclosure: string }) {
  if (s.kind === 'shared') return <SharedCard message={s.message} />
  if (s.kind === 'attachment')
    return (
      <span className="flex max-w-[85%] min-w-0 items-center gap-1 text-xs text-fg-secondary md:max-w-[70%]" title={s.text}>
        <Paperclip className="size-3 shrink-0" aria-hidden="true" />
        <span className="truncate">
          {s.from ? `${s.from} · ` : ''}
          {s.text || 'attachment'}
        </span>
      </span>
    )
  return s.text.trim() ? <LongText text={s.text} className={BUBBLE} disclosure={disclosure} /> : null
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

/** Card `i` of `n` gets its share of the message's images (`files`, `occurrences` or token paths alike): every image when there
 * is one card; else overview `i` plus, on the last card, every crop (96 §6.4 orders crops loops-first across displays, so a
 * crop's display is not recoverable — the release display gets them). */
export function imagesFor<T>(xs: T[] | undefined, i: number, n: number): T[] | undefined {
  if (!xs || n <= 1) return xs
  const own = xs[i] === undefined ? [] : [xs[i] as T]
  return i === n - 1 ? [...own, ...xs.slice(n)] : own
}

/** Right-aligned bubble; a full-width `SkillCard` above the words when the input was a skill injection. `turn` is set only on the message that opened a turn (steers carry none).
 * `context` is the stored row's membership (48 §3.3): its attachments are the authoritative occurrences (`files` is our optimistic list until then), and a removed row renders as archive.
 * `readOnly` (an inspection page) shows text and attachments only: no edit, fork or removal controls.
 * Leading `[attached …]`/`[doc …]` lines (66 §7.9) render as chips; the bubble shows the body only.
 * `channel` (172 D4) — or, for a row stored before 172, a leading `[slack dm]`/`[slack channel]` line — shows as an origin chip
 * and the marker line comes off; a Slack body's forwarded/linked/attachment blocks render as cards (172 D5).
 * `[screen …]` lines (96 §6.6) render as one `ScreenContextCard` per display under the words, carrying the message's images;
 * `[image: …]`/`[document: …]` tokens (a send from another webview, or history before `context` lands) resolve under `<data_dir>/attachments/`.
 * 171F: `cells` (the parked cell results sent with it) fold into one row above the bubble; long words fold (`LongText`);
 * both keep their open state under the entry `id`. */
export const MessageUser = memo(function MessageUser({
  id,
  text,
  skill,
  files,
  session,
  turn,
  context,
  channel,
  at,
  readOnly,
  cells,
}: {
  id?: string
  text: string
  skill?: string
  files?: string[]
  session: string
  turn?: number
  context?: ContextEntry
  channel?: ChannelOrigin
  at?: number
  readOnly?: boolean
  cells?: CellResultInput[]
}) {
  const api = useApi()
  const running = useSessionField(session, 'running')
  const fork = useFork(session)
  const edit = useDrafts((s) => s.edits[session])
  const editing = !readOnly && turn != null && edit?.turn === turn
  // 45 §7.4 / 66 §7.9: the markers are the text, so own sends and history render alike. 172: the Slack marker and an
  // allowed sender's `<from>: ` prefix are for the model; the chip shows both.
  const origin = originOf({ text, channel })
  const { refs, body } = parse(shownText({ text, channel }))
  const note = parseNote(body)
  // 76 talk: `[spoken, N s(, interrupted)]` is the user's utterance from the live talk — the same bubble under a caption.
  const spoken = note ? null : parseSpoken(body)
  // 71 §2.9: a `[board …]` header behind the markers is a place on a board — the quote card, not a bubble.
  const ask = note || spoken ? null : parseAsk(body)
  // 96 §0.11: the `[screen …]` lines and binary tokens come off the words; the lines are cards, the tokens thumbnails.
  const { body: words, screens, images } = parseScreenLines(note?.body ?? spoken?.text ?? body)
  const segments = origin?.via === 'slack' && words ? parseShared(words) : null
  const dataDir = useConnection((s) => s.info?.data_dir)
  const occurrences = context?.attachments
  // A token naming a MIME type (`[image: image/png]`, a part stored without a name) has no file to show.
  const paths = files ?? (occurrences || !dataDir ? undefined : images.filter((n) => !n.includes('/')).map((n) => `${dataDir}/attachments/${n}`))
  const stamp = useStampToggle()
  const actions = !readOnly && !editing && !context?.removed && (context || (turn != null && !running))
  return (
    <div className="group flex flex-col items-end gap-2 py-2" onClick={stamp.onClick}>
      {origin && <OriginChip origin={origin} />}
      {skill?.split(', ').map((name) => <SkillCard key={name} name={name} />)}
      {cells?.length ? <SentResults cells={cells} disclosure={disclosureKey(session, id, 'cells')} /> : null}
      {screens.length === 0 && <Attachments files={paths} occurrences={occurrences} session={readOnly ? undefined : session} active={context?.active} className="justify-end" />}
      {!editing && <ChipRow refs={refs} session={readOnly ? undefined : session} />}
      {note && (
        <span className="flex items-center gap-1 text-xs text-fg-secondary">
          <Mic className="size-3" /> {note.secs} s
        </span>
      )}
      {spoken && (
        <span className="flex items-center gap-1 text-xs text-fg-secondary">
          <AudioLines className="size-3" /> spoken · {spoken.secs} s{spoken.interrupted && ' · cut in'}
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
      ) : ask ? (
        <BoardQuoteCard ask={ask} />
      ) : segments ? (
        segments.map((s, i) => <SlackSegment key={i} segment={s} disclosure={disclosureKey(session, id, 'text', i)} />)
      ) : (
        words && <LongText text={words} className={BUBBLE} disclosure={disclosureKey(session, id, 'text')} />
      )}
      {!editing &&
        screens.map((s, i) => (
          <ScreenContextCard
            key={i}
            screen={s}
            files={imagesFor(paths, i, screens.length)}
            occurrences={imagesFor(occurrences, i, screens.length)}
            session={readOnly ? undefined : session}
            active={context?.active}
          />
        ))}
      {(at !== undefined || actions) && (
        <div className="-mr-2 flex items-center justify-end gap-2">
          {actions && (
            <div className="flex opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100 pointer-coarse:opacity-100">
              {body.trim() && <CopyButton text={body} />}
              {turn != null && !running && (
                <>
                  <ActionButton icon={Pencil} label="Edit" onClick={() => useDrafts.getState().beginEdit(session, { turn, text: body, refs })} />
                  <ActionButton icon={GitFork} label="Fork from here" onClick={() => void fork(turn)} />
                </>
              )}
              {context && <RemoveButton session={session} target={context.target} active={context.active} />}
            </div>
          )}
          <Stamp at={at} shown={stamp.shown} className="mr-2" />
        </div>
      )}
    </div>
  )
})
