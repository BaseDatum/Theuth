import { code } from '@streamdown/code'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Check, Copy, GitFork, type LucideIcon, RefreshCw, Square, Trash2, Volume2 } from 'lucide-react'
import { memo, useMemo, useState } from 'react'
import { toast } from 'sonner'
import { type PluginConfig, Streamdown } from 'streamdown'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { queryKeys, useApi } from '@/lib/rpc'
import * as speech from '@/lib/speech'
import { cn } from '@/lib/utils'
import type { ContextEntry } from '@/rpc/bindings/ContextEntry'
import type { ContextTarget } from '@/rpc/bindings/ContextTarget'
import { useSessionField, useSessions } from '@/store/sessions'
import { openContextPicker } from './ContextPicker'
import { MermaidFence } from './Mermaid'
import { FenceContext, JsonRenderFence } from './surface/JsonRenderFence'
import { OpenUIFence } from './surface/OpenUIFence'
import { Stamp, useStampToggle } from './Stamp'

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
export const shikiTheme: ['github-light', 'github-dark'] = ['github-light', 'github-dark']

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
 * `disabled` uses `aria-disabled` so the tooltip and focus still explain the button. */
export function ActionButton({
  icon: Icon,
  label,
  onClick,
  done,
  disabled,
  className,
}: {
  icon: LucideIcon
  label: string
  onClick?: () => void
  done?: boolean
  disabled?: boolean
  className?: string
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          className={cn('size-11 rounded-lg text-fg-secondary aria-disabled:opacity-50 aria-disabled:hover:bg-transparent', className)}
          aria-label={label}
          aria-disabled={disabled || undefined}
          onClick={disabled ? undefined : onClick}
        >
          {done ? <Check /> : <Icon />}
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
  className,
}: {
  session: string
  target: ContextTarget
  active: boolean
  icon?: LucideIcon
  label?: string
  className?: string
}) {
  const ok = useContextEditable(session)
  const revision = useSessionField(session, 'contextRevision')
  if (!active) return <ActionButton icon={icon} label="Already outside current context" disabled className={className} />
  return (
    <ActionButton
      icon={icon}
      label={label}
      disabled={!ok || revision == null}
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

/** Cut the transcript at `turn` optimistically, then run `rpc`; on failure toast and re-fetch the transcript (29 §3). */
export function rewind(session: string, turn: number, rpc: () => Promise<unknown>) {
  useSessions.getState().truncate(session, turn)
  rpc().catch((e: unknown) => {
    toast.error(e instanceof Error ? e.message : String(e))
    window.dispatchEvent(new Event('theuth:reload-session'))
  })
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
  // 30 §8: a later reply before the next human message carries the fix for any fence broken here.
  // ponytail: a forward scan per mounted reply per store change; index entries by id if transcripts grow past thousands.
  const hasLaterAssistant = useSessions((s) => {
    const entries = s.sessions[session]?.entries ?? []
    for (let i = entries.findIndex((e) => e.id === id) + 1; i > 0 && i < entries.length; i++) {
      const kind = entries[i]?.kind
      if (kind === 'assistant') return true
      if (kind === 'user') return false
    }
    return false
  })
  const fence = useMemo<FenceContext>(
    () => ({ running, hasLaterAssistant, session: readOnly ? undefined : session }),
    [running, hasLaterAssistant, readOnly, session],
  )
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
  return (
    <div className="group py-2" onClick={stamp.onClick}>
      <FenceContext value={fence}>
        <Streamdown
          mode={open ? undefined : 'static'}
          isAnimating={open}
          plugins={plugins}
          shikiTheme={shikiTheme}
          className="[&_h1]:mb-2 [&_h1]:text-2xl [&_h1]:leading-8 [&_h1]:font-semibold [&_h2]:mb-2 [&_h2]:mt-6 [&_h2]:text-xl [&_h2]:font-semibold [&_h3]:mt-4 [&_h3]:font-semibold [&_p]:my-1 [&_p]:leading-[26px] [&_li]:pt-3 [&_li]:leading-[26px] [&_a]:underline [&_a]:underline-offset-2 [&_pre]:rounded-2xl [&_table_th]:font-semibold"
        >
          {text}
        </Streamdown>
      </FenceContext>
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
          <Stamp at={at} shown={stamp.shown} className="ml-auto" />
        </div>
      )}
    </div>
  )
})
