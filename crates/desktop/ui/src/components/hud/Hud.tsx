import { useNavigate } from '@tanstack/react-router'
import { PanelLeft } from 'lucide-react'
import { useEffect, useMemo, useRef } from 'react'
import { Composer } from '@/components/chat/Composer'
import { simFlag } from '@/components/settings/sandbox-queries'
import { useRoutines } from '@/components/routines/queries'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { useAgents, useTasks } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useActivity, useActivityToday, useActivityWeek, useAttention, useSessionContexts, useSessionList, useSpendReport } from './queries'
import { sceneFixture } from './scene/fixtures'
import type { SceneData } from './scene/model'
import { SceneSwitch } from './scene/SceneCompact'

/**
 * `/` — the THEUTH mark's overview: the `Overview` header row (sidebar toggle) and the 128 scene, nothing else. Sky's
 * R2 ruling retired the sections that used to sit below it (Attention cards, In flight, Board pulse, Activity, Spend,
 * the docked composer): everything they reached is one click from the scene — /attention/$id, /agents/$name,
 * /tasks/$id, /tasks?state=…, /routines/$name, /new. 207: the Attention card and its actions live in components/attention.
 */
export function Hud() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const attention = useAttention()
  const tasks = useTasks()
  const agents = useAgents()
  const routines = useRoutines()
  const activity = useActivity()
  const activityToday = useActivityToday()
  const activityWeek = useActivityWeek()
  const sessions = useSessionList()
  const contexts = useSessionContexts(sessions.data)
  const spend = useSpendReport()
  useEffect(() => {
    document.title = ''
  }, [])
  // 128: the scene is a pure function of the data; under `?sim=` every source is the fixture (the activity and session hooks already are).
  const sim = simFlag()
  const fixture = useMemo(() => (sim ? sceneFixture(Date.now()) : null), [sim])
  const sceneData = useMemo<SceneData>(
    () => ({
      attention: fixture?.attention ?? attention.data,
      agents: fixture?.agents ?? agents.data,
      tasks: fixture?.tasks ?? tasks.data,
      routines: fixture?.routines ?? routines.data,
      sessions: fixture?.sessions ?? sessions.data,
      activityToday: activityToday.data,
      activityWeek: activityWeek.data,
      activity: fixture?.activity ?? activity.data,
      contexts: fixture?.contexts ?? contexts,
      spend: spend.data,
    }),
    [fixture, attention.data, agents.data, tasks.data, routines.data, sessions.data, activityToday.data, activityWeek.data, activity.data, contexts, spend.data],
  )
  const sceneErrors = (
    [
      ['attention', attention.error],
      ['agents', agents.error],
      ['tasks', tasks.error],
      ['routines', routines.error],
      ['sessions', sessions.error],
      ['activity today', activityToday.error],
      ['activity week', activityWeek.error],
      ['activity', activity.error],
      ['spend', spend.error],
    ] as const
  )
    .filter(([, e]) => e !== null)
    .map(([name]) => name)
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Overview</h2>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex min-h-full flex-col px-4 pt-4 pb-4">
          <SceneSwitch data={sceneData} errors={sceneErrors} />
        </div>
      </div>
      <div className="shrink-0 border-t border-border-subtle px-4 pt-3">
        <DeskComposer known={sessions.data?.find((s) => s.desk === 'theuth')?.id ?? null} />
      </div>
    </article>
  )
}

/**
 * R6 (Sky): the home composer talks to theuth's desk, not a new session — the voice key's route (lib/voice-key.ts). The
 * desk id comes from the sessions list when a `desk: 'theuth'` row is already there, else from `api.desk_open('theuth')`
 * on the first submit (cached for this mount; the composer stays enabled meanwhile and the send awaits it). A known desk
 * goes through `Composer`'s own steer/send_input path; either way a successful send opens `/c/<desk>` so the reply shows.
 */
function DeskComposer({ known }: { known: string | null }) {
  const api = useApi()
  const navigate = useNavigate()
  const cached = useRef<string | null>(null)
  const desk = known ?? cached.current
  const open = (session: string) => void navigate({ to: '/c/$session', params: { session } })
  if (desk !== null) return <Composer session={desk} onSent={open} compact />
  return (
    <Composer
      session={null}
      resolveSession={async () => {
        cached.current = await api.desk_open('theuth')
        return cached.current
      }}
      compact
    />
  )
}
