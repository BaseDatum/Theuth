// 76 §0.12 / §5.2: the talk view. Centre: the orb (the state) and two subtitle lines. Left: the session's
// transcript, collapsed by default, remembered per session. Right: every panel a session has, from `Header`
// exactly as in `/c/$session`. Phones: the transcript is a sheet. Leaving the view does not stop the talk.

import { createFileRoute, Link, useNavigate } from '@tanstack/react-router'
import { ChevronLeft, ChevronRight, MessageSquareText } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Composer } from '@/components/chat/Composer'
import { Header } from '@/components/chat/Header'
import { Transcript } from '@/components/chat/Transcript'
import { CodeMapPane } from '@/components/codemap/CodeMapHost'
import { Orb } from '@/components/talk/Orb'
import { Subtitles } from '@/components/talk/Subtitles'
import { Button } from '@/components/ui/button'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { useApi } from '@/lib/rpc'
import * as talk from '@/lib/talk'
import { cn } from '@/lib/utils'
import { useSessionLoad } from '@/routes/c.$session'
import { useSessionField } from '@/store/sessions'

export const Route = createFileRoute('/talk/$session')({ component: TalkRoute })

const PREF = 'talk-transcript'

/** Per-session "transcript open" choice; collapsed until the user opens it. */
function transcriptPref(session: string): boolean {
  try {
    return (JSON.parse(localStorage.getItem(PREF) ?? '{}') as Record<string, boolean>)[session] === true
  } catch {
    return false
  }
}

function saveTranscriptPref(session: string, open: boolean): void {
  try {
    const all = JSON.parse(localStorage.getItem(PREF) ?? '{}') as Record<string, boolean>
    all[session] = open
    localStorage.setItem(PREF, JSON.stringify(all))
  } catch {}
}

function TalkRoute() {
  const { session } = Route.useParams()
  const api = useApi()
  const navigate = useNavigate()
  const mobile = useIsMobile()
  const t = talk.useTalk()
  const title = useSessionField(session, 'title')
  const activity = useSessionField(session, 'activity')
  const otherTitle = useSessionField(t.session ?? '', 'title')
  useSessionLoad(session)
  const [open, setOpen] = useState(() => transcriptPref(session))
  useEffect(() => saveTranscriptPref(session, open), [session, open])
  useEffect(() => {
    document.title = `Talk${title ? ` · ${title}` : ''}`
  }, [title])

  // Item 7: `?sim=…` drives the store with a scripted sequence; the import is dead code in production.
  useEffect(() => {
    if (!import.meta.env.DEV) return
    let stop: (() => void) | undefined
    void import('@/lib/talk-sim').then((m) => {
      const flag = m.simFlag()
      if (flag) stop = m.runSim(session, flag)
    })
    return () => stop?.()
  }, [session])

  /** The talk runs on this session (started here or attached via `talk_status`). */
  const here = t.phase !== 'off' && t.session === session
  /** …or on another one: the daemon holds one talk. */
  const elsewhere = t.phase !== 'off' && t.session !== null && t.session !== session
  const state = here ? t.state : 'idle'
  const orb = talk.orbState(state)

  const toggle = () => {
    if (here) void talk.stop(api)
    else if (!elsewhere) void talk.start(api, session)
  }

  useEffect(() => {
    const onStop = () => {
      if (here) void talk.stop(api)
    }
    const onTranscript = () => setOpen((o) => !o)
    // §5.2: Esc while the agent thinks or speaks is the agent's cancel, as in the chat; the talk goes on listening.
    // TALK-UI-B B3: any other Esc leaves the view for the same session's chat (the talk keeps running there) — never a
    // blank view. Decided after the other Esc owners (a sheet, a dialog, a recording, dictation) had their turn.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return
      const el = e.target
      if (el instanceof HTMLElement && (el.isContentEditable || el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return
      if (here && state !== 'listening' && state !== 'idle') {
        void api.cancel(session)
        return
      }
      window.setTimeout(() => {
        if (e.defaultPrevented || (mobile && open) || document.querySelector(':is([role="dialog"], [role="alertdialog"], [role="menu"])[data-state="open"]')) return
        void navigate({ to: '/c/$session', params: { session } })
      }, 0)
    }
    window.addEventListener('theuth:talk', onStop)
    window.addEventListener('theuth:talk-transcript', onTranscript)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('theuth:talk', onStop)
      window.removeEventListener('theuth:talk-transcript', onTranscript)
      window.removeEventListener('keydown', onKey)
    }
  }, [api, here, session, state, navigate, open, mobile])

  // The orb is the centre at every width: 60 % of the narrower side of its pane, 160–440 px.
  const centre = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState(240)
  useEffect(() => {
    const el = centre.current
    if (!el) return
    const measure = () => setSize(Math.round(Math.min(440, Math.max(160, Math.min(el.clientWidth * 0.6, el.clientHeight * 0.5)))))
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const caption = here ? t.degraded : null
  // A2: the turn's phase, muted, above the orb (`talk.phaseLine`).
  const phase = here ? talk.phaseLine(state, activity === 'tool', t.subs.agent !== '') : ''
  // A4: what the tap does now, below the subtitles.
  const hint = elsewhere ? null : talk.orbHint(state)
  const label = here ? 'Stop talking' : elsewhere ? `Talking in ${otherTitle || 'another chat'}` : 'Start talking'

  const transcript = (
    <>
      <Transcript key={`t:${session}`} session={session} />
      <Composer key={`c:${session}`} session={session} readOnly={here} />
    </>
  )

  return (
    <CodeMapPane session={session}>
      <Header session={session} popout={{ kind: 'talk', session }} />
      <div className="flex min-h-0 min-w-0 flex-1 flex-row">
        {!mobile && open && (
          <aside className="flex min-h-0 w-[min(440px,42vw)] shrink-0 flex-col border-r border-border-subtle" aria-label="Transcript">
            {transcript}
          </aside>
        )}
        {!mobile && (
          <button
            type="button"
            aria-label={open ? 'Hide transcript' : 'Show transcript'}
            aria-expanded={open}
            title={`${open ? 'Hide' : 'Show'} transcript (⌘⇧T)`}
            onClick={() => setOpen((o) => !o)}
            className="flex w-4 shrink-0 cursor-pointer items-center justify-center text-fg-tertiary/50 transition-colors hover:bg-layer-1 hover:text-fg-secondary focus-visible:text-fg-secondary focus-visible:outline-none"
          >
            {open ? <ChevronLeft className="size-4" /> : <ChevronRight className="size-4" />}
          </button>
        )}
        <div ref={centre} className="relative flex min-h-0 min-w-0 flex-1 flex-col items-center justify-center gap-10 px-4 pb-8">
          <p aria-live="polite" className={cn('h-[1lh] text-center text-xs text-fg-tertiary transition-opacity duration-150 motion-reduce:transition-none', phase ? 'opacity-100' : 'opacity-0')}>
            {phase}
          </p>
          <Orb state={orb} size={size} onClick={toggle} label={label} />
          <Subtitles agent={here ? t.subs.agent : ''} user={here ? t.subs.user : ''} caption={caption} />
          <p className={cn('h-[1lh] text-center text-xs text-fg-tertiary transition-opacity duration-150 motion-reduce:transition-none', hint ? 'opacity-100' : 'opacity-0')}>
            {hint}
          </p>
          {elsewhere && t.session && (
            <Link
              to="/talk/$session"
              params={{ session: t.session }}
              className="absolute bottom-6 text-xs text-fg-tertiary underline-offset-4 hover:text-fg-secondary hover:underline"
            >
              Talking in {otherTitle || 'another chat'} — open
            </Link>
          )}
          {mobile && (
            <Button
              variant="ghost"
              size="sm"
              className={cn('absolute bottom-3 left-3 rounded-full text-fg-tertiary', elsewhere && 'hidden')}
              aria-label="Show transcript"
              onClick={() => setOpen(true)}
            >
              <MessageSquareText className="size-4" /> Transcript
            </Button>
          )}
        </div>
      </div>
      {mobile && (
        <Sheet open={open} onOpenChange={setOpen}>
          <SheetContent side="bottom" showCloseButton={false} className="top-0 flex h-band flex-col gap-0 pt-[var(--safe-top)]">
            <SheetTitle className="sr-only">Transcript</SheetTitle>
            <div className="flex h-12 shrink-0 items-center justify-between px-3">
              <span className="text-sm font-medium">Transcript</span>
              <SheetClose asChild>
                <Button variant="ghost" size="sm" className="rounded-full">
                  Done
                </Button>
              </SheetClose>
            </div>
            {transcript}
          </SheetContent>
        </Sheet>
      )}
    </CodeMapPane>
  )
}
