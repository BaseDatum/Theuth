import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { ExternalLink, MessageSquarePlus, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useApi } from '@/lib/rpc'
import { LiveSession } from '@/routes/c.$session'
import { startRoutineChat } from './draft'

const EXAMPLES = [
  { label: 'Weekday brief', text: 'Every weekday at 9, brief me on overnight mail, today’s calendar and open PRs. Quietly — just message me the summary.' },
  { label: 'PR babysitter', text: 'Watch PR #482 on skymoore/wth every minute for two weeks: when CI fails, read the log and push a fix; when a review lands, address it. Stop after 40 runs.' },
  { label: 'YouTube channel', text: 'When Veritasium posts a new video, summarise it in five bullets and message me. Check every 10 minutes for 90 days.' },
]

/**
 * 108 §6.1 — the "Describe it" pane: an intro box until a session exists, then the chat page's `LiveSession` with
 * `first={false}` (the meetings precedent) under a thin row with close / open-in-chat. `seed` is Edit by chat (§6.6).
 */
export function RoutineChat({
  session,
  onSession,
  seed,
  onClose,
}: {
  session: string | null
  onSession: (id: string) => void
  seed?: { name: string; text: string }
  onClose: () => void
}) {
  const api = useApi()
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const send = async () => {
    const words = text.trim()
    if (!words || busy) return
    setBusy(true)
    try {
      await startRoutineChat(api, words, seed, onSession)
    } catch (e) {
      toast.error(errorText(e))
      setBusy(false)
    }
  }
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border-subtle px-2 text-xs text-fg-secondary">
        <span className="min-w-0 flex-1 truncate px-1">{seed ? `Edit ${seed.name} by chat` : 'Describe it'}</span>
        {session && (
          <Button asChild variant="ghost" size="xs">
            <Link to="/c/$session" params={{ session }} search={{}}>
              Open in chat <ExternalLink />
            </Link>
          </Button>
        )}
        <Button variant="ghost" size="xs" onClick={onClose} aria-label="Close chat">
          <X /> Close chat
        </Button>
      </div>
      {session ? (
        <LiveSession session={session} first={false} />
      ) : (
        <div className="flex flex-col gap-3 p-4">
          <p className="text-sm text-fg-secondary">
            {seed ? `What should change about ${seed.name}? The current file goes along with your words.` : 'Describe the routine — when it should run, what it should do, when it should stop. The model asks only what it must and proposes a draft you can create or edit in the form.'}
          </p>
          <Textarea
            rows={5}
            autoFocus
            value={text}
            placeholder={seed ? 'Run at 8 instead, and skip Fridays' : 'Every weekday at 9, brief me on…'}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void send()
            }}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" disabled={!text.trim() || busy} onClick={() => void send()}>
              <MessageSquarePlus /> Describe it
            </Button>
            {!seed &&
              EXAMPLES.map((x) => (
                <Button key={x.label} type="button" variant="outline" size="xs" onClick={() => setText(x.text)}>
                  {x.label}
                </Button>
              ))}
          </div>
        </div>
      )}
    </div>
  )
}

/**
 * §6.5 D9: a `routine_draft` card pressed "Open in form" in an ordinary chat (`/c/<id>`) lands on `/routines/new?chat=<id>`;
 * the page then reads the draft from the slice already in the store. Mounted once in `__root.tsx`; the Routines page
 * handles its own pane's presses, so this only fires away from `/routines`.
 */
export function RoutineDraftBridge() {
  const navigate = useNavigate()
  const { location } = useRouterState()
  useEffect(() => {
    const hear = (e: Event) => {
      const d = (e as CustomEvent<{ session: string; app: string; message: string }>).detail
      if (d.app !== 'routine-drafts' || !/^routine draft r[0-9]+(?:-[a-z0-9]{1,4})?: open in form$/m.test(d.message)) return
      if (location.pathname.startsWith('/routines/')) return
      void navigate({ to: '/routines/new', search: { chat: d.session } })
    }
    window.addEventListener('theuth:app-press', hear)
    return () => window.removeEventListener('theuth:app-press', hear)
  }, [navigate, location.pathname])
  return null
}
