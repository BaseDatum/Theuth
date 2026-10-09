// 144 §10, §12.1 (wave 2, U2): the Overviews list under the notes column — one row per kind (newest version), its
// status, Open / Regenerate, older versions behind `▾ versions`; the audio row plays `overview.mp3` inline.
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { AudioLines, CalendarRange, ChevronRight, FileText, type LucideIcon, MessageCircleQuestion, Network, RefreshCw } from 'lucide-react'
import { toast } from 'sonner'
import { useFileUrl } from '@/components/chat/Attachments'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { askKeys, useAskApi, useOverviews } from '@/lib/ask-rpc'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Overview } from '@/rpc/bindings/Overview'
import type { OverviewKind } from '@/rpc/bindings/OverviewKind'

/** The five kinds in menu order (§10): glyph and label shared by the header's Overviews ▾ menu. */
export const OVERVIEWS: readonly { kind: OverviewKind; label: string; Icon: LucideIcon }[] = [
  { kind: 'briefing', label: 'Briefing', Icon: FileText },
  { kind: 'faq', label: 'FAQ', Icon: MessageCircleQuestion },
  { kind: 'timeline', label: 'Timeline', Icon: CalendarRange },
  { kind: 'mind_map', label: 'Mind map', Icon: Network },
  { kind: 'audio', label: 'Audio overview', Icon: AudioLines },
]

/** The audio kind lands two artifacts per version (contract §2); the script is listed under the player, not as a row of its own. */
export const SCRIPT = 'overview-script.md'
const isScript = (o: Overview) => o.kind === 'audio' && o.name === SCRIPT

/** Pure: rows of one kind, newest version first, the script rows set aside. */
export function versionsOf(rows: readonly Overview[], kind: OverviewKind): Overview[] {
  return rows.filter((o) => o.kind === kind && !isScript(o)).sort((a, b) => b.version - a.version)
}

/** `notebook_overview_run(notebook, kind)` → toast; the list polls until the row leaves `running`. */
export function useRunOverview(notebook: string) {
  const api = useAskApi()
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (kind: OverviewKind) => api.notebook_overview_run(notebook, kind),
    onSuccess: (_r, kind) => {
      toast(`Generating ${OVERVIEWS.find((o) => o.kind === kind)?.label.toLowerCase() ?? kind}…`)
      void qc.invalidateQueries({ queryKey: askKeys.overviews(notebook) })
    },
    onError: (e) => toast.error(e.message),
  })
}

export function OverviewsSection({ notebook }: { notebook: string }) {
  const q = useOverviews(notebook)
  const run = useRunOverview(notebook)
  const rows = q.data ?? []
  const kinds = OVERVIEWS.map((k) => ({ ...k, versions: versionsOf(rows, k.kind) })).filter((k) => k.versions.length > 0)
  return (
    <section className="border-t border-border-subtle" aria-label="Overviews">
      <header className="flex h-10 shrink-0 items-center gap-2 px-3 text-xs font-semibold">Overviews</header>
      {q.isError && <p className="px-3 pb-3 text-xs text-danger">{q.error.message}</p>}
      {q.isSuccess && kinds.length === 0 && (
        <p className="px-3 pb-3 text-xs text-fg-tertiary">No overviews yet — Overviews ▾ in the header generates a briefing, FAQ, timeline, mind map or an audio overview.</p>
      )}
      <ul className="flex flex-col gap-1 px-2 pb-2">
        {kinds.map(({ kind, label, Icon, versions }) => {
          const [current, ...older] = versions as [Overview, ...Overview[]]
          const script = rows.find((o) => isScript(o) && o.version === current.version) ?? null
          return (
            <li key={kind} className="rounded-[10px] px-1.5 py-1.5 hover:bg-layer-2">
              <Row o={current} label={label} Icon={Icon} script={script} regenerate={() => run.mutate(kind)} busy={run.isPending} />
              {older.length > 0 && (
                <Collapsible className="group/versions mt-1">
                  <CollapsibleTrigger className="flex items-center gap-1 pl-6 text-[11px] text-fg-tertiary">
                    <ChevronRight className="size-3 transition-transform group-data-[state=open]/versions:rotate-90" aria-hidden />
                    versions · {older.length}
                  </CollapsibleTrigger>
                  <CollapsibleContent>
                    <ul className="flex flex-col gap-0.5 pt-1 pl-6">
                      {older.map((o) => (
                        <li key={`${o.session}/${o.version}`} className="flex items-center gap-2 text-[11px] text-fg-tertiary">
                          <span className="min-w-0 flex-1 truncate">
                            {o.name} · v{o.version} · {when(o)}
                          </span>
                          <Status o={o} />
                          <Open o={o} />
                        </li>
                      ))}
                    </ul>
                  </CollapsibleContent>
                </Collapsible>
              )}
            </li>
          )
        })}
      </ul>
    </section>
  )
}

const when = (o: Overview) => ago(new Date(Number(o.created_ms)).toISOString())

function Row({ o, label, Icon, script, regenerate, busy }: { o: Overview; label: string; Icon: LucideIcon; script: Overview | null; regenerate(): void; busy: boolean }) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2 text-xs">
        <Icon className="size-4 shrink-0 text-fg-tertiary" aria-label={label} />
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-semibold">{label}</span>
          <span className="truncate text-[11px] text-fg-tertiary">
            {o.name} · v{o.version} · {when(o)}
          </span>
        </span>
        <Status o={o} />
        <Open o={o} />
        <Button variant="ghost" size="icon-xs" aria-label={`Regenerate ${label.toLowerCase()}`} title="Regenerate" disabled={busy || o.status === 'running'} onClick={regenerate}>
          <RefreshCw />
        </Button>
      </div>
      {o.kind === 'audio' && o.status === 'ready' && o.artifact && (
        <div className="flex flex-col gap-1 pl-6">
          <Audio callId={o.artifact} />
          {o.note && <span className="text-[11px] text-fg-tertiary">{o.note}</span>}
          {script && (
            <span className="flex items-center gap-2 text-[11px] text-fg-tertiary">
              <FileText className="size-3" aria-hidden />
              <span className="min-w-0 flex-1 truncate">{script.name}</span>
              <Open o={script} />
            </span>
          )}
        </div>
      )}
    </div>
  )
}

function Status({ o }: { o: Overview }) {
  if (o.status === 'running') return <span className="shrink-0 text-[11px] text-fg-tertiary">running…</span>
  if (o.status === 'failed')
    return (
      <span className="min-w-0 max-w-[40%] truncate text-[11px] text-danger" title={o.error ?? undefined}>
        failed{o.error ? ` · ${o.error}` : ''}
      </span>
    )
  return <span className="shrink-0 text-[11px] text-fg-tertiary">ready</span>
}

/** The artifact viewer (design 59 / wave 1's `/artifacts/$callId`); nothing to open until the `show` landed. */
function Open({ o }: { o: Overview }) {
  if (!o.artifact) return null
  return (
    <Button asChild variant="ghost" size="xs">
      <Link to="/artifacts/$callId" params={{ callId: o.artifact }}>
        Open
      </Link>
    </Button>
  )
}

/** `<audio controls>` over the mp3's bytes: `asset://` on this Mac, else `get_attachment` → blob URL (31 §9) — the same path the viewer streams by. */
function Audio({ callId }: { callId: string }) {
  const api = useApi()
  const record = useQuery({ queryKey: queryKeys.artifact(callId), queryFn: () => api.get_artifact(callId) })
  if (!record.data) return <span className="h-8 w-full animate-pulse rounded-full bg-layer-2" />
  return <Player path={record.data.path} />
}

function Player({ path }: { path: string }) {
  const src = useFileUrl(path)
  if (src === null) return <span className="h-8 w-full animate-pulse rounded-full bg-layer-2" />
  if (src === '') return <span className="text-[11px] text-danger">could not load the audio</span>
  return <audio controls preload="metadata" src={src} className="h-8 w-full" />
}
