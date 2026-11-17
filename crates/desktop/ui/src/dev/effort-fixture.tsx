// 194 live proof, dev only: `bunx vite` then open `/src/dev/effort-fixture.html?row=<3..20>` (`&theme=dark`). The real
// picker of one contract §4 inventory row, against no engine: `dev/local-fixture.ts` answers the new 194 effort RPCs
// (a chip pick round-trips), `endless-fixture` / `routines-fixture` their areas, and `base` below the rest — the
// catalog has models with effort levels, every model setting starts on one, and a whole-struct setter stores its value.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { Component, type ReactNode, StrictMode, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { AgentDetail } from '@/components/agents/AgentDetail'
import { RuntimeFields } from '@/components/agents/RuntimeAccessFields'
import { type Draft, emptyDraft } from '@/components/agents/wizard'
import { LeafModel } from '@/components/autoresearch/DraftPanel'
import { MemoryPanel } from '@/components/chat/MemoryPanel'
import { ModelMenu } from '@/components/chat/ModelMenu'
import { StartForm } from '@/components/meeting/StartForm'
import { RoutineDetail } from '@/components/routines/RoutineDetail'
import { Advisor } from '@/components/settings/Advisor'
import { Autoresearch } from '@/components/settings/Autoresearch'
import { Dreaming } from '@/components/settings/Dreaming'
import { Helpers } from '@/components/settings/Helpers'
import { MeetingDefaults } from '@/components/settings/Meetings'
import { Memory } from '@/components/settings/Memory'
import { ModelDefaults } from '@/components/settings/ModelDefaults'
import { Dictation } from '@/components/settings/Voice'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { Api } from '@/rpc/bindings/client'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { autoresearchFixture } from '@/components/autoresearch/fixtures'
import { SidebarProvider } from '@/components/ui/sidebar'
import { endlessFixture } from '@/dev/endless-fixture'
import { localFixture } from '@/dev/local-fixture'

for (const [f, v] of Object.entries({ 'local-fixture': '1', 'endless-fixture': '1', 'routines-fixture': '1', 'dreaming-fixture': 'night', 'autoresearch-fixture': 'studies' })) localStorage.setItem(f, v)
const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
const row = Number(query.get('row') ?? 3)

const model = (id: string, name: string, efforts: string[], context = 200_000): ModelInfo => ({ id, name, provider: id.split('/')[0]!, context, reasoning: efforts.length > 0, image: true, efforts, budget_min: null, budget_max: null, local: false })
const OPUS = model('anthropic/claude-opus-4-8', 'Claude Opus 4.8', ['low', 'medium', 'high', 'xhigh', 'max'], 1_000_000)
const SONNET = model('anthropic/claude-sonnet-4-6', 'Claude Sonnet 4.6', ['low', 'medium', 'high'])
const GPT = model('openai/gpt-5.4', 'GPT-5.4', ['minimal', 'low', 'medium', 'high'])
const PLAIN = model('openai/gpt-4.1', 'GPT-4.1', [])
// The ids the endless / dreaming / autoresearch fixtures name, given levels so their rows have chips too.
const HAIKU = model('anthropic/claude-haiku-4-5', 'Claude Haiku 4.5', ['low', 'medium', 'high'])
const SOL = model('openai/gpt-5.6-sol', 'GPT-5.6 Sol', ['low', 'medium', 'high', 'xhigh'])
const CATALOG = [OPUS, SONNET, HAIKU, GPT, SOL, PLAIN]

/** Every setting's store: a getter returns it, a setter replaces it and returns it (a pick round-trips). */
const s: Record<string, unknown> = {
  subagent_model: SONNET,
  theuth_agent_model: OPUS,
  title_model: SONNET,
  vision_model: GPT,
  escalate_model: { model: OPUS, resolves_to: OPUS },
  polish_model: SONNET,
  voice: { shape: { composer: 'plain', tui: 'full', companion: 'full', phone: 'off', any_app: 'full' }, end_commands: { send: ['send it'], scratch: ['scratch that'], literal: 'verbatim' }, input_devices: [], standby_secs: 8, duck_media: 'off', polish_model: SONNET.id, polish_profiles: {}, history: { enabled: false, rows: 200, audio_mib: 64, learn: false } },
  meeting: { model: GPT.id, effort: null, summary_model: SONNET.id, summary_effort: null, template: 'meeting', language: 'en', live_diarization: true, live_tools: [], project: null, detect: { enabled: true, ignore: [], poll_secs: 2 }, calendar: { eventkit: false, prebrief: true, poll_secs: 900 } },
}
const info = {
  default_model: OPUS.id,
  default_effort: 'high',
  favorite_models: [OPUS.id, GPT.id],
  advisor: { model: GPT.id, effort: null, max_notes_per_update: 4, immune_turns: 2, sync_backlog: '1', max_consults: 6 },
}
const byId = (id: string | null) => (id ? (CATALOG.find((m) => m.id === id) ?? null) : null)
const ok = <T,>(v: T) => Promise.resolve(v)
const base: Record<string, (...a: never[]) => Promise<unknown>> = {
  info: () => ok(info),
  list_models: () => ok(CATALOG),
  set_model_favorite: (id: string, on: boolean) => ok((info.favorite_models = on ? [...info.favorite_models, id] : info.favorite_models.filter((x) => x !== id))),
  subagent_model: () => ok(s.subagent_model),
  set_subagent_model: (id: string | null) => ok((s.subagent_model = byId(id))),
  theuth_agent_model: () => ok(s.theuth_agent_model),
  set_theuth_agent_model: (id: string | null) => ok((s.theuth_agent_model = byId(id))),
  title_model: () => ok(s.title_model),
  set_title_model: (id: string | null) => ok((s.title_model = byId(id))),
  vision_model: () => ok(s.vision_model),
  set_vision_model: (id: string | null) => ok((s.vision_model = byId(id))),
  escalate_model: () => ok(s.escalate_model),
  set_escalate_model: (id: string | null) => ok((s.escalate_model = { model: byId(id), resolves_to: byId(id) ?? s.title_model })),
  polish_model: () => ok(s.polish_model),
  set_polish_model: (id: string | null) => ok((s.polish_model = byId(id))),
  meeting_settings_get: () => ok(s.meeting),
  meeting_settings_set: (v: never) => ok((s.meeting = v)),
  meeting_templates: () => ok([]),
  list_sessions: () => ok([]),
  list_project_styles: () => ok([]),
  agent_get: (name: string) => ok({ name, title: name, reports_to: 'user', model: OPUS.id, effort: null, tools: 'destructive', mcp: null, budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, runs_in: 'any', body: 'Standing instructions (prompt.md).' }),
  agents_list: () => ok([]),
  list_acp: () => ok([]),
  list_mcp: () => ok([]),
  voice_settings: () => ok(s.voice),
  voice_status: () => ok({ model: { state: 'ready' }, accelerator: '', voice: 'af', voices: [], speaking: null, queued: [], input_devices: [] }),
  dictation_status: () => ok({ model: { state: 'ready' }, active: null, input_device: null, last: null }),
  dictation_history: () => ok([]),
  list_input_devices: () => ok([{ name: 'MacBook Pro Microphone', default: true }]),
  sandboxes_list: () => ok([]),
  meeting_processes: () => ok([]),
  meeting_permissions: () => ok({ microphone: 'Authorized', audio_capture: 'Authorized' }),
  meeting_list: () => ok([]),
  list_schedules: () => ok([]),
  mesh_status: () => ok(null),
  tasks_list: () => ok([]),
  'autoresearch.mechanism': () => ok({ active: null, versions: [] }),
}
// The target is `base` itself: the fixture wrappers spread the `Api` (`{ ...api }`), which copies only own keys.
const api = new Proxy(base as unknown as Api, {
  get: (t, name: string) =>
    (t as unknown as typeof base)[name] ??
    (async () => {
      console.warn(`fixture: ${name} not answered`)
      throw new Error(`fixture: ${name} not answered`)
    }),
})
// `legacy` discovery: no engine to probe, so every capability-gated query runs against `base`.
active().connection.setState({ api, client: {} as RpcClient, info: info as never, discovery: { ...active().connection.getState().discovery, phase: 'legacy' } })
// The driver reads failed queries here (a method `base` lacks fails as "not a function" inside a wrapper).
Object.assign(window, { qc: active().qc })
// Rows 11 and 19 start on a model with levels (their fixtures default to "Same as …", which has no chips). Each fixture
// keeps one state per wrapped `Api`, so the presets go through the object `useApi()` hands the components.
const endless = endlessFixture(api)
await endless.endless_settings_set({ ...(await endless.endless_settings_get()), model: OPUS.id })
const research = autoresearchFixture(localFixture(endless))
const roles = await research['autoresearch.settings.get']()
await research['autoresearch.settings.set']({ ...roles, models: { ...roles.models, executor: OPUS.id, advisor: GPT.id } })

/** One row's failure shows its error instead of a blank page. */
class Guard extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null }
  static getDerivedStateFromError(error: Error) {
    return { error }
  }
  render() {
    return this.state.error ? <pre className="text-sm text-destructive">{String(this.state.error.stack)}</pre> : this.props.children
  }
}

function AgentRuntime() {
  const [d, setD] = useState<Draft>({ ...emptyDraft(), name: 'research', model: OPUS.id, memory_model: SONNET.id } as Draft)
  return <RuntimeFields d={d} set={(k, v) => setD((x) => ({ ...x, [k]: v }))} />
}

function Leaf() {
  const [values, setValues] = useState<Record<string, string>>({ 'models.executor': OPUS.id, 'models.executor_effort': '' })
  return (
    <div className="flex flex-col gap-1">
      <span className="font-mono text-xs text-fg-tertiary">models.executor</span>
      <LeafModel leaf="models.executor" values={values} setValues={setValues} models={CATALOG} />
    </div>
  )
}

const ROWS: Record<number, { title: string; body: () => ReactNode }> = {
  3: { title: 'Settings › Models › Subagents', body: () => <ModelDefaults /> },
  4: { title: 'Settings › Models › theuth agent', body: () => <ModelDefaults /> },
  5: {
    title: 'Agents › theuth › Runtime',
    body: () => (
      <SidebarProvider>
        <AgentDetail name="theuth" tab="runtime" />
      </SidebarProvider>
    ),
  },
  6: { title: 'Settings › Advisor', body: () => <Advisor /> },
  7: { title: 'Settings › Helpers › Title', body: () => <Helpers /> },
  8: { title: 'Settings › Helpers › Vision fallback', body: () => <Helpers /> },
  9: { title: 'Settings › Helpers › Escalation', body: () => <Helpers /> },
  10: { title: 'Settings › Voice › Polish', body: () => <Dictation /> },
  11: { title: 'Settings › Memory › Compactor', body: () => <Memory /> },
  12: { title: 'Chat › Memory panel › Summary model', body: () => <MemoryPanel session="s1" open onOpenChange={() => undefined} /> },
  13: { title: 'Agents › Memory summary model', body: () => <AgentRuntime /> },
  14: { title: 'Settings › Meetings › Summary model', body: () => <MeetingDefaults /> },
  15: { title: 'Settings › Dreaming › tier models', body: () => <Dreaming /> },
  16: { title: 'Settings › Meetings › Meeting model', body: () => <MeetingDefaults /> },
  17: { title: 'Meeting start › Session model', body: () => <StartForm /> },
  18: {
    title: 'Routines › Model',
    body: () => (
      <SidebarProvider>
        <RoutineDetail name={query.get('routine') ?? 'standup'} section="overview" chat={undefined} />
      </SidebarProvider>
    ),
  },
  19: { title: 'Settings › Autoresearch › role models', body: () => <Autoresearch /> },
  20: { title: 'Autoresearch draft › leaf models', body: () => <Leaf /> },
  // Row 21 (audit): the chat header's picker for a new chat; on touch it opens the top-of-band dialog (194 D1).
  21: { title: 'Chat header (new chat)', body: () => <ModelMenu session={null} /> },
}

function Fixture() {
  const r = ROWS[row]
  return (
    <TooltipProvider>
      <div className="h-dvh overflow-y-auto">
        <div className="mx-auto flex max-w-[768px] flex-col gap-4 px-4 py-6">
          <p className="font-mono text-xs text-fg-tertiary">
            194 row {row}: {r?.title ?? 'unknown row'}
          </p>
          <Guard>{r?.body()}</Guard>
        </div>
      </div>
    </TooltipProvider>
  )
}

const router = createRouter({ routeTree: createRootRoute({ component: Fixture }), history: createMemoryHistory() })

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={active().qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
