import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { ExternalLink } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { SectionRow } from '@/components/nav/SectionRow'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { useSessions } from '@/store/sessions'
import { DraftPanel } from './DraftPanel'
import { useDraft, useDrafts, useStudies, useStudy } from './queries'
import { SetupChat } from './SetupChat'
import { CARD_PRESS, DRAFTS_APP, basename, lastSetupResultId, latestStudyDraft, recentRepos } from './setup'
import { StatePill } from './StudyList'

/**
 * 151 §4.7 `SetupPane` — `/autoresearch/new`: the 44 px header (back, title, state chip, repo chip, "Open as full
 * chat"), the embedded `SetupChat` and the live `DraftPanel` beside it. `chat` is the setup session (`?chat=<id>`);
 * without it the intro box shows. Phone: the chat is the page, the panel a second sheet from the "Draft n of m" pill.
 */
export function SetupPane({ chat }: { chat: string | undefined }) {
  const navigate = useNavigate()
  const mobile = useIsMobile()
  const [panelOpen, setPanelOpen] = useState(false)
  const session = chat ?? null
  const studies = useStudies()
  const drafts = useDrafts()
  const repos = useMemo(() => recentRepos(studies.data ?? [], drafts.data ?? []), [studies.data, drafts.data])

  // D3: the newest `autoresearch_draft` call in the session names the draft; the newest draft/probe *result* refetches it.
  const entries = useSessions((s) => (session ? s.sessions[session]?.entries : undefined))
  const ref = useMemo(() => (entries ? latestStudyDraft(entries) : null), [entries])
  const resultId = useMemo(() => (entries ? lastSetupResultId(entries) : null), [entries])
  // A rail row opened before the agent re-drafted in this session: the drafts list knows the draft of this origin.
  const draftId = ref?.draftId ?? drafts.data?.find((d) => d.origin === session)?.draft_id ?? null
  const draft = useDraft(draftId, resultId)
  const started = draft.data?.started === true
  const study = useStudy(draftId ?? '', started)
  const info = study.data?.info
  const repo = draft.data?.spec.repo

  const panel = <DraftPanel draft={draft.data} session={session} onDiscarded={() => void navigate({ to: '/autoresearch', search: {} })} className="h-full" />
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionRow
        back={{ to: '/autoresearch', label: 'Studies', className: 'md:hidden' }}
        title={
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate">{draftId ?? 'New study'}</span>
            {info ? <StatePill state={info.state} currentN={info.current} stop={info.stop_reason} /> : <span className="inline-flex h-5 shrink-0 items-center rounded-full border border-info-border/40 bg-info-bg px-2 text-[11px] font-medium whitespace-nowrap text-info">Setting up with theuth</span>}
            {repo && (
              <span className="hidden h-5 shrink-0 items-center rounded-full border border-border px-2 font-mono text-[11px] text-fg-secondary lg:inline-flex" title={repo}>
                {basename(repo)}
              </span>
            )}
          </span>
        }
      >
        {session && (
          <Button asChild variant="ghost" size="xs" className="ml-auto">
            <Link to="/c/$session" params={{ session }} search={{}}>
              Open as full chat <ExternalLink />
            </Link>
          </Button>
        )}
      </SectionRow>
      <div className="flex min-h-0 min-w-0 flex-1">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <SetupChat
            session={session}
            repos={repos}
            onSession={(chat) => void navigate({ to: '/autoresearch/new', search: { chat } })}
            aboveComposer={
              // R1 H4: the pill sits right above the composer, clear of the assistant FAB at the bottom edge.
              mobile && session ? (
                <div className="flex shrink-0 justify-center py-1">
                  <Button variant="outline" size="xs" className="rounded-full" onClick={() => setPanelOpen(true)}>
                    {draft.data ? (draft.data.started ? `Study ${draft.data.draft_id}` : `Draft ${draft.data.settled} of ${draft.data.total}`) : 'Draft'}
                  </Button>
                </div>
              ) : undefined
            }
          />
        </div>
        {!mobile && session && (
          // R1 U1: a fixed-width panel so rows read at 1280 px; the chat takes the rest.
          <aside className="flex min-h-0 shrink-0 flex-col border-l border-border-subtle bg-layer-1" style={{ width: 'clamp(320px, 28vw, 400px)' }}>
            {panel}
          </aside>
        )}
      </div>
      {mobile && (
        <Sheet open={panelOpen} onOpenChange={setPanelOpen}>
          <SheetContent side="bottom" className="flex h-[85dvh] flex-col gap-0 p-0">
            <SheetTitle className="sr-only">Study draft</SheetTitle>
            {panel}
          </SheetContent>
        </Sheet>
      )}
    </div>
  )
}

/**
 * §4.8: the compact card's "Open in Autoresearch" pressed in an ordinary chat (`/c/<id>`) lands on
 * `/autoresearch/new?chat=<id>`. Mounted once in `__root.tsx` beside `RoutineDraftBridge`; under `/autoresearch/new`
 * the pane already shows that session, so this only fires elsewhere.
 */
export function StudyDraftBridge() {
  const navigate = useNavigate()
  const { location } = useRouterState()
  useEffect(() => {
    const hear = (e: Event) => {
      const d = (e as CustomEvent<{ session: string; app: string; message: string }>).detail
      if (d.app !== DRAFTS_APP || !CARD_PRESS.test(d.message)) return
      if (location.pathname.startsWith('/autoresearch/new')) return
      void navigate({ to: '/autoresearch/new', search: { chat: d.session } })
    }
    window.addEventListener('theuth:app-press', hear)
    return () => window.removeEventListener('theuth:app-press', hear)
  }, [navigate, location.pathname])
  return null
}
