// 144 §9 (wave 2, U2): `/ask/$session` — one ask-across-notebooks run drawn from its `NotebookAsk` events (the plan
// as steps, the branches as collapsible rows with their hit chips and streaming partial answers, the end), over the
// `kind: ask` session's normal chat (transcript + composer) so the user keeps asking. Nothing is replayed: after a
// reload the structure is gone and the session's own turns carry the answer.
import { useMutation, useQuery } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { ArrowRight, CircleX, ExternalLink, RefreshCw } from 'lucide-react'
import { useEffect } from 'react'
import { Streamdown } from 'streamdown'
import { toast } from 'sonner'
import { CitationsProvider, References } from '@/components/chat/Citations'
import { Composer } from '@/components/chat/Composer'
import { markdownComponents, plugins, rehypePlugins, shikiTheme } from '@/components/chat/MessageAssistant'
import { Transcript } from '@/components/chat/Transcript'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { useSources } from '@/components/notebooks/useNotebook'
import { useNotebooks } from '@/components/notebooks/useNotebooks'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { type AskState, askKeys, hitHref, useAskApi, useAskEvents } from '@/lib/ask-rpc'
import { useApi } from '@/lib/rpc'
import type { AskBranch } from "@/rpc/bindings/event.agent_2enotebook_2dask.output"
import type { AskHit } from "@/rpc/bindings/event.agent_2enotebook_2dask.output"
import type { Notebook } from "@/rpc/bindings/method.agent.output"
import { useSessions } from '@/store/sessions'

const route = getRouteApi('/ask_/$session')
const PAGE = 60

export function AskView() {
  const { session } = route.useParams()
  return <Ask key={session} session={session} />
}

/** The session page, once (what `ChatColumn` does for a notebook thread). */
function useLoadSession(session: string) {
  const api = useApi()
  useEffect(() => {
    const st = useSessions.getState()
    st.ensure(session)
    if (st.sessions[session]?.loaded) return
    api.load_session(session, null, PAGE).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
    )
  }, [api, session])
}

/** Pure: the hits of a branch as the §6.1 chip grammar, one link per hit, for the normal message renderer. */
export function hitsMarkdown(hits: readonly AskHit[], sourceIds: ReadonlySet<string>): string {
  return hits.map((h) => `[${h.title.replace(/[[\]]/g, ' ') || h.id}](${hitHref(h, sourceIds.has(h.id) ? 'sources' : 'notes')})`).join(' · ')
}

/** Pure: the plan as steps — one per branch: its notebooks' titles, its query. */
export function planSteps(plan: readonly AskBranch[], notebooks: readonly Notebook[]): { title: string; detail: string }[] {
  return plan.map((b) => ({ title: b.notebooks.map((id) => notebooks.find((n) => n.doc.id === id)?.doc.title || 'Untitled').join(' · '), detail: b.query }))
}

function Ask({ session }: { session: string }) {
  const api = useAskApi()
  const navigate = useNavigate()
  useLoadSession(session)
  const state = useAskEvents(session)
  const notebooks = useNotebooks(false).data ?? []
  const sourceIds = new Set((useSources(null).data ?? []).map((s) => s.doc.id))
  // The run's own row (question, scope) is what survives a reload and what Retry re-sends.
  const summary = useQuery({ queryKey: askKeys.list, queryFn: () => api.notebook_ask_list(null) }).data?.find((a) => a.session === session)
  // The synthesis is the session's assistant turn; the `answer` event only bridges until that turn is in the transcript.
  const answered = useSessions((s) => (s.sessions[session]?.entries ?? []).some((e) => e.kind === 'assistant'))
  const retry = useMutation({
    mutationFn: () => api.notebook_ask(summary?.scope ?? [], summary?.question ?? '', null),
    onSuccess: (r) => void navigate({ to: '/ask/$session', params: { session: r.session } }),
    onError: (e) => toast.error(e.message),
  })
  const title = summary?.question ?? 'Ask across notebooks'
  useEffect(() => {
    document.title = title
  }, [title])
  const live = state.plan !== null && state.done === null
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <SectionRow back={{ to: '/notebooks', label: 'Back to notebooks' }} title={title}>
        {summary && (
          <span className="shrink-0 text-xs text-fg-tertiary">
            {summary.scope.length} {summary.scope.length === 1 ? 'notebook' : 'notebooks'} · {ago(new Date(Number(summary.created_ms)).toISOString())} · {summary.done ? 'done' : 'running'}
          </span>
        )}
        <Button asChild variant="ghost" size="icon-sm" aria-label="Open in the chat list" title="Open in the chat list">
          <Link to="/c/$session" params={{ session }} search={{}}>
            <ExternalLink />
          </Link>
        </Button>
      </SectionRow>
      <AskStructure state={state} notebooks={notebooks} sourceIds={sourceIds} answered={answered} live={live} retry={summary ? () => retry.mutate() : null} retrying={retry.isPending} />
      <Transcript key={session} session={session} compact />
      <div className="shrink-0 border-t border-border-subtle bg-background">
        <Composer session={session} sessionTitle={title} compact />
      </div>
    </article>
  )
}

/** The run drawn from its events (exported for the render test); nothing until the plan or an early `done` arrived. */
export function AskStructure({ state, notebooks, sourceIds, answered, live, retry, retrying }: { state: AskState; notebooks: readonly Notebook[]; sourceIds: ReadonlySet<string>; answered: boolean; live: boolean; retry: (() => void) | null; retrying: boolean }) {
  if (state.plan === null && state.done === null) return null
  const plan = state.plan ?? []
  return (
    <div className="flex max-h-[55%] shrink-0 flex-col gap-3 overflow-y-auto border-b border-border-subtle px-4 py-3 text-sm" aria-label="Ask run">
      {plan.length > 0 && (
        <div role="list" aria-label="Plan" className="flex flex-col gap-2 rounded-xl border border-border-subtle p-3">
          {planSteps(plan, notebooks).map((it, i) => (
            <div key={i} role="listitem" className="flex gap-3">
              <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-fg-accent text-xs font-semibold text-primary-foreground tabular-nums">{i + 1}</span>
              <div className="flex min-w-0 flex-col pt-0.5">
                <div className="font-medium">{it.title}</div>
                <div className="text-fg-secondary">{it.detail}</div>
              </div>
            </div>
          ))}
        </div>
      )}
      {plan.length > 0 && (
        <div className="flex flex-col divide-y divide-border-subtle">
          {plan.map((b) => {
            const hits = state.hits[b.n]
            const partial = state.partial[b.n]
            return (
              <Collapsible key={b.n} defaultOpen className="group/acc py-2">
                <CollapsibleTrigger className="flex w-full items-center gap-2 text-left text-sm font-medium">
                  <ArrowRight className="size-3.5 shrink-0 text-fg-tertiary transition-transform group-data-[state=open]/acc:rotate-90" aria-hidden />
                  <span className="min-w-0 flex-1 truncate">
                    Branch {b.n} · {b.query}
                  </span>
                  <span className="shrink-0 text-xs font-normal text-fg-tertiary">{hits === undefined ? (live ? 'searching…' : '') : `${hits.length} ${hits.length === 1 ? 'hit' : 'hits'}`}</span>
                </CollapsibleTrigger>
                <CollapsibleContent className="flex flex-col gap-2 pt-2 pl-5.5">
                  {hits && hits.length > 0 && <Markdown text={hitsMarkdown(hits, sourceIds)} />}
                  {partial ? <Markdown text={partial} /> : hits !== undefined && live ? <span className="text-xs text-fg-tertiary">answering…</span> : null}
                </CollapsibleContent>
              </Collapsible>
            )
          })}
        </div>
      )}
      {state.answer !== null && !answered && <Markdown text={state.answer} />}
      {state.done?.error != null && (
        <div role="alert" className="flex items-start gap-2 rounded-xl border border-danger-border bg-danger-bg px-4 py-3 text-sm text-danger">
          <CircleX className="mt-0.5 size-4 shrink-0" aria-hidden />
          <div className="min-w-0 flex-1 whitespace-pre-wrap text-foreground">{state.done.error}</div>
          {retry && (
            <Button variant="outline" size="sm" disabled={retrying} onClick={retry}>
              <RefreshCw /> Retry
            </Button>
          )}
        </div>
      )}
    </div>
  )
}

/** Markdown with §6 chips through the chat's renderer (`MessageAssistant`'s pieces), static. */
function Markdown({ text }: { text: string }) {
  return (
    <CitationsProvider text={text}>
      <Streamdown mode="static" plugins={plugins} rehypePlugins={rehypePlugins} components={markdownComponents} shikiTheme={shikiTheme} className="[&_p]:my-1 [&_a]:underline [&_a]:underline-offset-2">
        {text}
      </Streamdown>
      <References />
    </CitationsProvider>
  )
}
