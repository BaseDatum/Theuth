// 215 live proof, dev only: `bunx vite` then open `/src/dev/composer-fixture.html` (`?theme=dark`; `?open=360` opens
// section 5's popover in the narrow column instead of the wide one). The REAL `Composer`, `ComposerModel` and
// `ModelCombobox` against no engine, each at 768px and 360px: a stub `Api` answers what they ask (a catalog with effort
// levels and one local model, the engine's default model/effort, a skill index), and the stores they read are seeded —
// a live session's model, a draft's text, two parked skills (`useSkillDraft`) and an active prompt suffix (`useSuffixes`).
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { Component, type ReactNode, StrictMode, useEffect, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Composer } from '@/components/chat/Composer'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { ComposerModel } from '@/components/chat/ModelMenu'
import { modelRequirements } from '@/components/chat/requirements'
import { Row } from '@/components/settings/shared'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useDrafts, useSkillDraft } from '@/lib/drafts'
import { useSuffixes } from '@/lib/suffixes'
import type { Api } from '@/rpc/bindings/client'
import type { ModelInfo, SkillMeta } from '@/rpc/bindings/method.agent.output'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { useSessions } from '@/store/sessions'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
const openAt = Number(query.get('open') ?? 768)

const model = (id: string, name: string, efforts: string[], local = false): ModelInfo => ({ id, name, provider: id.split('/')[0]!, context: 200_000, reasoning: efforts.length > 0, image: true, efforts, budget_min: null, budget_max: null, local })
const OPUS = model('anthropic/claude-opus-4-8', 'Claude Opus 4.8', ['low', 'medium', 'high', 'xhigh', 'max'])
const SONNET = model('anthropic/claude-sonnet-4-6', 'Claude Sonnet 4.6', ['low', 'medium', 'high'])
const GPT = model('openai/gpt-5.4', 'GPT-5.4', ['minimal', 'low', 'medium', 'high'])
const QWEN = model('ollama/qwen3-coder:30b', 'Qwen3 Coder 30B', [], true)
const CATALOG = [OPUS, SONNET, GPT, QWEN]
const SKILLS = [
  { name: 'release-notes', description: 'Draft release notes from the merged PRs since the last tag.', always: false, path: '/skills/release-notes/SKILL.md' },
  { name: 'code-review-with-a-deliberately-long-name', description: 'Review the diff for correctness and maintainability.', always: false, path: '/skills/code-review/SKILL.md' },
] as unknown as SkillMeta[]

/** Live sessions (uuids, as the engine names them). */
const LIVE = '00000000-0000-4000-8000-0000000c0de1'
const PILLS = '00000000-0000-4000-8000-0000000c0de2'
const COMPACT = '00000000-0000-4000-8000-0000000c0de3'

const info = { default_model: OPUS.id, default_effort: 'high', favorite_models: [OPUS.id, GPT.id] }
const ok = <T,>(v: T) => Promise.resolve(v)
const base: Record<string, (...a: never[]) => Promise<unknown>> = {
  info: () => ok(info),
  list_models: () => ok(CATALOG),
  list_skills: () => ok(SKILLS),
  vision_model: () => ok(null),
  list_sessions: () => ok([]),
  set_model_favorite: (id: string, on: boolean) => ok((info.favorite_models = on ? [...info.favorite_models, id] : info.favorite_models.filter((x) => x !== id))),
  voice_status: () => ok({ model: { state: 'ready' }, accelerator: '', voice: 'af', voices: [], speaking: null, queued: [], input_devices: [] }),
  list_mcp: () => ok([]),
}
const api = new Proxy(base as unknown as Api, {
  get: (t, name: string) =>
    (t as unknown as typeof base)[name] ??
    (async () => {
      console.warn(`fixture: ${name} not answered`)
      throw new Error(`fixture: ${name} not answered`)
    }),
})
// A `ready` discovery advertising exactly what `base` answers plus the rest of the model methods (`ComposerModel` is
// behind a `Capability`, which legacy discovery never satisfies) and the event topic: every other gated query stays off.
const methods = [...modelRequirements.methods, 'list_skills', 'use_skill', 'use_skills', 'vision_model', 'voice_status', 'list_mcp', 'set_mcp_hidden', 'subscribe_topics', 'unsubscribe_topics']
const discovery = {
  methods: methods.map((name) => ({ name, params: [], result: {} })),
  'x-theuth': { subscriptions: [{ subscribe: 'subscribe_topics', unsubscribe: 'unsubscribe_topics', notification: 'subscribe_topics' }], events: [{ topic: 'agent.events', version: 1 }] },
} as never
active().connection.setState({ api, client: {} as RpcClient, info: info as never, discovery: { phase: 'ready', capabilities: null, document: discovery, unavailable: new Set() } })

// The live sessions' model/effort (what a `load_session` would have put in the store).
for (const [id, m, effort] of [[LIVE, OPUS.id, 'high'], [PILLS, SONNET.id, 'medium'], [COMPACT, QWEN.id, null]] as const) {
  useSessions.getState().ensure(id)
  useSessions.setState((s) => ({ sessions: { ...s.sessions, [id]: { ...s.sessions[id]!, model: m, effort } } }))
}
// Section 2's typed text; the drafts are keyed by session (a panel's by `panel:<session>`).
useDrafts.getState().setText(LIVE, 'Summarise the failing CI run on main.\nThen propose a fix for the flaky test,\nand list anything else that looks wrong.')
useDrafts.getState().setText(PILLS, 'Write the notes for v0.42.')
for (const s of SKILLS) useSkillDraft.getState().add(PILLS, s.name)
useSuffixes.setState({
  list: [{ id: 'sfx-terse', name: 'Be terse and cite file paths', text: 'Answer in at most five bullets; cite every file path.' }],
  active: { [PILLS]: { id: 'sfx-terse', scope: 'always' } },
})

/** One section's failure shows its error instead of a blank page. */
class Guard extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null }
  static getDerivedStateFromError(error: Error) {
    return { error }
  }
  render() {
    return this.state.error ? <pre className="text-sm text-destructive">{String(this.state.error.stack)}</pre> : this.props.children
  }
}

/** Section 5: the trigger clicked once after mount, as a user would (late enough that no composer's refocus closes it). */
function OpenModel({ open }: { open: boolean }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const t = setTimeout(() => ref.current?.querySelector('button')?.click(), 600)
    return () => clearTimeout(t)
  }, [open])
  return (
    <div ref={ref} className="flex justify-end pt-[420px]">
      <ComposerModel session={null} slash={false} />
    </div>
  )
}

function Field() {
  const [value, setValue] = useState<string | null>(SONNET.id)
  const [effort, setEffort] = useState<string | null>('medium')
  const current = CATALOG.find((m) => m.id === value)
  return (
    <Row stack label="Subagents" description="The model a subagent starts on.">
      <ModelCombobox
        aria-label="Subagent model"
        className="max-w-64 max-md:w-full max-md:max-w-none"
        models={CATALOG}
        value={value}
        onChange={setValue}
        effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: effort, onSelect: setEffort } : undefined}
        none={{ label: 'Same as the chat' }}
      />
    </Row>
  )
}

const SECTIONS: { title: string; body: (width: number) => ReactNode }[] = [
  { title: '1. Composer — new chat (session=null), empty', body: () => <Composer session={null} /> },
  { title: '2. Composer — live session, typed multi-line text', body: () => <Composer session={LIVE} /> },
  { title: '3. Composer — two parked skills + an "every message" prompt suffix', body: () => <Composer session={PILLS} /> },
  { title: '4. Composer — compact (notebook chat column: compact + ownModel)', body: () => <Composer session={COMPACT} compact ownModel /> },
  { title: '5. ComposerModel — popover open (new chat)', body: (w) => <OpenModel open={w === openAt} /> },
  { title: '6. ModelCombobox — settings field beside its label', body: () => <Field /> },
]

function Fixture() {
  return (
    <TooltipProvider>
      <div className="h-dvh overflow-y-auto">
        <div className="flex flex-col gap-10 px-6 py-6">
          <p className="font-mono text-xs text-fg-tertiary">215 composer fixture — real components, stub engine</p>
          {SECTIONS.map((s) => (
            <section key={s.title} className="flex flex-col gap-3">
              <h2 className="font-mono text-xs text-fg-secondary">{s.title}</h2>
              <div className="flex flex-wrap items-start gap-8">
                {[768, 360].map((w) => (
                  <div key={w} data-width={w} className="flex flex-col gap-1" style={{ width: w }}>
                    <span className="font-mono text-[11px] text-fg-tertiary">{w}px</span>
                    <div className="rounded-lg border border-dashed border-border-subtle p-2">
                      <Guard>{s.body(w)}</Guard>
                    </div>
                  </div>
                ))}
              </div>
            </section>
          ))}
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
