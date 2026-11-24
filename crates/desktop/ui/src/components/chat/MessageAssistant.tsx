import { code } from '@streamdown/code'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Check, Copy, FilePlus, GitFork, type LucideIcon, RefreshCw, Square, StickyNote, Trash2, Volume2, X } from 'lucide-react'
import { type ComponentProps, type ComponentType, memo, useContext, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { type Components, type CustomRendererProps, type PluginConfig, Streamdown, defaultRehypePlugins } from 'streamdown'
import type { PluggableList } from 'unified'
import { useNotebookChat, type NotebookChat } from '@/components/notebooks/NotebookContext'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useNotebooksApi } from '@/lib/notebooks-rpc'
import { routed } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import * as speech from '@/lib/speech'
import { cn } from '@/lib/utils'
import type { ContextEntry } from "@/rpc/bindings/method.agent.output"
import type { ContextTarget } from "@/rpc/bindings/method.agent.output"
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import { useSessionField, useSessions } from '@/store/sessions'
import { CitationLink, CitationsProvider, References } from './Citations'
import { openContextPicker } from './ContextPicker'
import { MermaidFence } from './Mermaid'
import { Reserve } from './Reserve'
import { RemovedFence, ShakeFenceContext, pieceOf, shakeFences } from './ShakeFence'
import { FenceContext, JsonRenderFence } from './surface/JsonRenderFence'
import { OpenUIFence } from './surface/OpenUIFence'
import { hasLaterAssistant, ownTurnRunning } from './surface/fence-turn'
import { holdFenceOpener } from './surface/streaming'
import { MarkdownTd } from './surface/registry'
import { Stamp, useStampToggle } from './Stamp'
import { CELL_LANGUAGES, CellFence, CellOrdinals, cellOrdinals } from '@/components/cells/CellFence'
import { FenceText } from './disclosure'

// Module scope: a new object per render would re-initialise the highlighter.
// `json-render`, `mermaid` and `openui` fences use streamdown's `plugins.renderers` (matched by language before the
// built-in code block), so no `components.code` override is needed and every other language keeps the `@streamdown/code`
// renderer. Mermaid itself is configured in `Mermaid.tsx` (30 §11) and rendered there, not by streamdown.
export const plugins: PluginConfig = {
  code,
  renderers: [
    { language: 'json-render', component: JsonRenderFence },
    { language: 'mermaid', component: MermaidFence },
    { language: 'openui', component: OpenUIFence },
  ],
}
/** 171 §4.1: assistant replies only add `CellFence` for shell and python fences; every other surface keeps `plugins`.
 * 197 §8: a reply's surface fences also carry their removal control (`shakeable`). */
const assistantPlugins: PluginConfig = {
  ...plugins,
  renderers: [...(plugins.renderers ?? []).map((r) => ({ ...r, component: shakeable(r.component) })), { language: CELL_LANGUAGES, component: CellFence }],
}

/** `Inner` with its corner `Remove from context` or, once removed, the collapsed marker (197 §8); a reply without
 * matching fence metadata (`ShakeFenceContext` null) renders `Inner` alone. */
function shakeable(Inner: ComponentType<CustomRendererProps>): ComponentType<CustomRendererProps> {
  return function ShakeableFence(props: CustomRendererProps) {
    const ctx = useContext(ShakeFenceContext)
    const piece = ctx && !props.isIncomplete ? pieceOf(ctx, props.language, props.code) : null
    if (!ctx || !piece) return <Inner {...props} />
    if (piece.removed) return <RemovedFence piece={piece} index={ctx.pieces.indexOf(piece)} inner={<Inner {...props} />} />
    return (
      <div className="group/fence relative">
        <Inner {...props} />
        <RemoveButton
          session={ctx.session}
          target={piece.target}
          active={ctx.active}
          icon={X}
          label={`Remove from context: ${piece.label}`}
          className="absolute right-0 bottom-0 rounded-xl text-fg-primary opacity-0 transition-opacity group-hover/fence:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100 [&_svg]:size-6 [&_svg]:rounded-full [&_svg]:bg-layer-1/90 [&_svg]:p-1 [&_svg]:shadow-sm"
        />
      </div>
    )
  }
}
/** 135 §2: a markdown image has no intrinsic size until it loads; `Reserve` holds its last height meanwhile. */
function Img({ node: _node, src, alt, ...rest }: ComponentProps<'img'> & { node?: unknown }) {
  const { entry } = useContext(FenceContext)
  const ref = useRef<HTMLImageElement>(null)
  const [ready, setReady] = useState(false)
  // A cached image may be complete before `onLoad` can fire.
  useLayoutEffect(() => {
    if (ref.current?.complete) setReady(true)
  }, [])
  const done = () => setReady(true)
  return (
    <Reserve id={entry && src ? `${entry}/${src}` : null} ready={ready} className="my-4">
      <img ref={ref} src={src} alt={alt} className="max-w-full rounded-lg" onLoad={done} onError={done} {...rest} />
    </Reserve>
  )
}
// Markdown tables (`| a | b |`) take streamdown's `td` with the json-render `Table`'s wrapping rules; the table itself goes
// natural-width via the `[&_table]` variants on the Streamdown className below, so streamdown's `overflow-x-auto` wrapper scrolls.
export const markdownComponents: Components = { td: MarkdownTd, img: Img, a: CitationLink }
export const shikiTheme: ['github-light', 'github-dark'] = ['github-light', 'github-dark']
/**
 * 144 §6: streamdown's sanitizer keeps only http/https/mailto-class hrefs and `harden` then prints `[blocked]` for a
 * link without one. `theuth://` citations need the scheme on the allow-list; everything else stays the default.
 */
const sanitize = defaultRehypePlugins.sanitize as [unknown, { protocols?: { href?: string[] } }]
export const rehypePlugins = Object.values({
  ...defaultRehypePlugins,
  sanitize: [sanitize[0], { ...sanitize[1], protocols: { ...sanitize[1].protocols, href: [...(sanitize[1].protocols?.href ?? []), 'theuth'] } }],
}) as PluggableList

/** `text` fenced with `language`; the fence outruns any backtick run inside. */
export function fenced(text: string, language: string): string {
  const longest = Math.max(2, ...[...text.matchAll(/`+/g)].map((m) => m[0].length))
  const fence = '`'.repeat(longest + 1)
  return `${fence}${language}\n${text}\n${fence}`
}

/** The last fenced code block's body, or `null`. */
export function lastCode(text: string): string | null {
  const blocks = [...text.matchAll(/```[^\n]*\n([\s\S]*?)```/g)]
  return blocks.at(-1)?.[1] ?? null
}

/** 44 px ghost icon button with a tooltip (ChatGPT's action bar); `done` swaps the icon for a check.
 * `disabled` uses `aria-disabled` so the tooltip and focus still explain the button. `text` also shows the label
 * beside the icon (a labelled button where an icon alone would be ambiguous, e.g. the lightbox's removal control). */
export function ActionButton({
  icon: Icon,
  label,
  onClick,
  done,
  disabled,
  text,
  className,
}: {
  icon: LucideIcon
  label: string
  onClick?: () => void
  done?: boolean
  disabled?: boolean
  text?: boolean
  className?: string
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          className={cn(text ? 'h-11 px-3' : 'size-11', 'rounded-lg text-fg-secondary aria-disabled:opacity-50 aria-disabled:hover:bg-transparent', className)}
          aria-label={label}
          aria-disabled={disabled || undefined}
          onClick={disabled ? undefined : onClick}
        >
          {done ? <Check /> : <Icon />}
          {text && label}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/** Whether the session takes a context edit right now (48 §2.3): idle, natively editable, and the loaded history is current. The server rechecks. */
export function useContextEditable(session: string): boolean {
  const running = useSessionField(session, 'running')
  const editable = useSessionField(session, 'contextEditable')
  const stale = useSessionField(session, 'contextStale')
  return !running && !!editable && !stale
}

/** `Remove from context` for one exact stored target (48 §2.3): opens the picker's review on it, pinned to the revision
 * this transcript was loaded at so a seq reused after an edit/fork is never retargeted (48 §3.1). An entry folded before
 * `tail_from` has no active target and says so; busy/stale/external sessions keep the button but disable it. */
export function RemoveButton({
  session,
  target,
  active,
  icon = Trash2,
  label = 'Remove from context',
  text,
  className,
}: {
  session: string
  target: ContextTarget
  active: boolean
  icon?: LucideIcon
  label?: string
  text?: boolean
  className?: string
}) {
  const ok = useContextEditable(session)
  const revision = useSessionField(session, 'contextRevision')
  if (!active) return <ActionButton icon={icon} label="Already outside current context" disabled text={text} className={className} />
  return (
    <ActionButton
      icon={icon}
      label={label}
      disabled={!ok || revision == null}
      text={text}
      className={className}
      onClick={() => revision != null && openContextPicker(session, undefined, [target], revision)}
    />
  )
}

/** Copy `text` and show a check for two seconds. */
export function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }) {
  const [done, setDone] = useState(false)
  return (
    <ActionButton
      icon={Copy}
      label={label}
      done={done}
      onClick={() => {
        void navigator.clipboard.writeText(text)
        setDone(true)
        setTimeout(() => setDone(false), 2000)
      }}
    />
  )
}

/** Cut the transcript at `turn` optimistically, then run `rpc`; on failure toast and re-fetch the transcript (29 §3).
 * 183 §4.4 (R1): through `routed` — an unreachable owner parks the call behind the card, and the transcript comes back
 * (nothing was cut there). */
export function rewind(session: string, turn: number, rpc: () => Promise<unknown>) {
  const reload = () => window.dispatchEvent(new Event('theuth:reload-session'))
  useSessions.getState().truncate(session, turn)
  routed(session, async () => (await rpc(), true)).then(
    (ran) => ran || reload(),
    (e: unknown) => {
      toast.error(e instanceof Error ? e.message : String(e))
      reload()
    },
  )
}

/** `fork(turn, reply?)`: copy `session` through `turn` (or through its `reply`-th text), refresh the list, open the copy. */
export function useFork(session: string) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  return async (turn: number, reply?: number) => {
    try {
      const id = await api.fork_session(session, turn, reply ?? null)
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
      await navigate({ to: '/c/$session', params: { session: id } })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
}

/** Position of assistant entry `id` among the replies of its turn: the server's `reply` index (29 §3); `undefined` (whole turn) if unknown. */
function replyIndex(session: string, turn: number, id: string): number | undefined {
  const entries = useSessions.getState().sessions[session]?.entries ?? []
  const n = entries.filter((e) => e.kind === 'assistant' && e.turn === turn).findIndex((e) => e.id === id)
  return n < 0 ? undefined : n
}

/**
 * 144 §8: in a notebook's chat, an answer can be saved as a new note or promoted (appended) into one of the notebook's notes.
 * §4 (Sky's ruling): the Save toast's **Open** carries `?nb`, so the note keeps the way back to this notebook.
 */
function NotebookActions({ session, turn, nbc }: { session: string; turn: number; nbc: NotebookChat }) {
  const nb = useNotebooksApi()
  const navigate = useNavigate()
  const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))
  const save = () =>
    nb.save_answer_as_note(session, turn, nbc.notebook).then((saved) => {
      toast('Saved as a note', { action: { label: 'Open', onClick: () => void navigate({ to: '/notes/$id', params: { id: saved.doc }, search: { nb: nbc.notebook } }) } })
      nbc.onChanged()
    }, fail)
  const promote = (note: DocSummary) =>
    nb.promote_answer(session, turn, note.id).then(() => {
      toast(`Promoted into “${note.title}”`)
      nbc.onChanged()
    }, fail)
  return (
    <>
      <ActionButton icon={StickyNote} label="Save answer as note" onClick={() => void save()} />
      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" className="size-11 rounded-lg text-fg-secondary" aria-label="Promote into a note">
                <FilePlus />
              </Button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent>Promote into a note</TooltipContent>
        </Tooltip>
        <DropdownMenuContent align="start" className="w-64">
          {nbc.notes.length === 0 ? (
            <DropdownMenuItem disabled>No notes in this notebook yet</DropdownMenuItem>
          ) : (
            nbc.notes.slice(0, 12).map((note) => (
              <DropdownMenuItem key={note.id} onSelect={() => void promote(note)}>
                <span className="truncate">{note.title}</span>
              </DropdownMenuItem>
            ))
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  )
}

/** Markdown at column width; `open` while the reply streams (only the last entry ever is). `turn` is the opener seq the reply belongs to; `id` is the entry id (for the fork's reply index).
 * `context` is the stored row's membership (48 §3.3); a removed row renders as archive, without turn actions. `readOnly` (an inspection page) keeps only copy/read-aloud. */
export const MessageAssistant = memo(function MessageAssistant({
  id,
  text,
  open,
  session,
  turn,
  context,
  at,
  readOnly,
}: {
  id: string
  text: string
  open: boolean
  session: string
  turn?: number
  context?: ContextEntry
  at?: number
  readOnly?: boolean
}) {
  const api = useApi()
  const running = useSessionField(session, 'running') ?? false
  // fence-skel D2: a fence's state follows the turn that wrote it, never a later one.
  const ownRunning = useSessions((s) => {
    const slice = s.sessions[session]
    return !!slice && ownTurnRunning(slice.entries, id, slice.running, slice.turnSeq)
  })
  // ponytail: a forward scan per mounted reply per store change; index entries by id if transcripts grow past thousands.
  const laterAssistant = useSessions((s) => hasLaterAssistant(s.sessions[session]?.entries ?? [], id))
  const fence = useMemo<FenceContext>(
    () => ({ running: ownRunning, hasLaterAssistant: laterAssistant, session: readOnly ? undefined : session, entry: id }),
    [ownRunning, laterAssistant, readOnly, session, id],
  )
  // 197 §8: the stored row's fences, offered only once settled and when their count matches the fences in `text`.
  const shake = useMemo(
    () => (readOnly || open || !context || context.removed ? null : shakeFences(session, context.active, text, context.fences)),
    [readOnly, open, context, session, text],
  )
  // 171 §3.4: a chat cell is `(session, stored message seq, ordinal)`; `turnRunning` holds cells of the reply whose turn runs.
  const turnSeq = useSessionField(session, 'turnSeq')
  const stored = context?.target.kind === 'message' ? String(context.target.seq) : null
  const turnRunning = running && (turn === undefined || turnSeq === undefined || turn === turnSeq)
  const ordinals = useMemo(() => cellOrdinals(text, stored, open || turnRunning), [text, stored, open, turnRunning])
  const fork = useFork(session)
  const voice = speech.useVoice()
  const speaking = speech.useSpeaking()
  const mine = speaking?.entry === id
  const readAloud = () => {
    if (speaking && mine) {
      speech.stop()
      speech.setSpeaking(null)
      void api.stop_speaking(speaking.id)
    } else {
      speech.prime()
      speech.speak(api, session, id, text).catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
    }
  }
  const stamp = useStampToggle()
  const nbc = useNotebookChat()
  // 171F B13 (T4): streamdown's static and streaming modes are different trees, so switching when the stream closes (the
  // first tool call) would remount every fence and cell in the reply mid-turn. Decided once per mount: a reply mounted
  // while it streams stays in streaming mode (settled, it renders the same); one mounted settled (a reload) is static.
  const [mode] = useState(open ? undefined : ('static' as const))
  return (
    <div className="group py-2" onClick={stamp.onClick}>
      <CitationsProvider text={text}>
        <FenceContext value={fence}>
          <FenceText value={text}>
            <CellOrdinals value={ordinals}>
              <ShakeFenceContext value={shake}>
                {/* The chat register only: every prose selector composes with `outside-openui` (styles.css), which
                    compiles each to `<root> <el>:not([data-streamdown="openui"] *)` — the markdown rhythm never
                    reaches inside an OpenUI fence this reply hosts, at any nesting depth. */}
                <Streamdown
                  mode={mode}
                  isAnimating={open}
                  plugins={assistantPlugins}
                  rehypePlugins={rehypePlugins}
                  components={markdownComponents}
                  shikiTheme={shikiTheme}
                  className="[&_h1]:outside-openui:mb-2 [&_h1]:outside-openui:text-2xl [&_h1]:outside-openui:leading-8 [&_h1]:outside-openui:font-semibold [&_h2]:outside-openui:mb-2 [&_h2]:outside-openui:mt-6 [&_h2]:outside-openui:text-xl [&_h2]:outside-openui:font-semibold [&_h3]:outside-openui:mt-4 [&_h3]:outside-openui:font-semibold [&_p]:outside-openui:my-1 [&_p]:outside-openui:leading-[26px] [&_li]:outside-openui:pt-3 [&_li]:outside-openui:leading-[26px] [&_a]:outside-openui:underline [&_a]:outside-openui:underline-offset-2 [&_pre]:outside-openui:rounded-2xl [&_table]:outside-openui:w-max [&_table]:outside-openui:min-w-full [&_table_th]:outside-openui:font-semibold [&_table_th]:outside-openui:wrap-normal"
                >
                  {open ? holdFenceOpener(text) : text}
                </Streamdown>
              </ShakeFenceContext>
            </CellOrdinals>
          </FenceText>
        </FenceContext>
        <References />
      </CitationsProvider>
      {!open && (
        <div className="-ml-2 flex items-center">
          {voice && <ActionButton icon={mine ? Square : Volume2} label={mine ? 'Stop' : 'Read aloud'} onClick={readAloud} />}
          <CopyButton text={text} />
          {!readOnly && turn != null && !running && !context?.removed && (
            <>
              <ActionButton icon={RefreshCw} label="Regenerate" onClick={() => rewind(session, turn, () => api.regenerate(session, turn))} />
              <ActionButton icon={GitFork} label="Fork from here" onClick={() => void fork(turn, replyIndex(session, turn, id))} />
            </>
          )}
          {!readOnly && context && !context.removed && <RemoveButton session={session} target={context.target} active={context.active} />}
          {nbc && !readOnly && turn != null && <NotebookActions session={session} turn={turn} nbc={nbc} />}
          <Stamp at={at} shown={stamp.shown} className="ml-auto" />
        </div>
      )}
    </div>
  )
})
