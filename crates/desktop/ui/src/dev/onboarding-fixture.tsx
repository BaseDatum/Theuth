// 215 visual check, dev only: `bunx vite` then open `/src/dev/onboarding-fixture.html`. No engine: the fake api below
// answers what onboarding reads, every other call fails. Never bundled (not a build input), like the other fixtures.
//   ?step=welcome|model|default-model|first-chat   the wizard at that step (default welcome)
//   ?models=empty                                   no model connected yet: Connect shows its three paths (default:
//                                                   a configured catalog)
//   ?login=pending                                  with ?step=model: Claude's sign-in waiting on the browser
//   ?dialog=phone                                   Link your phone
//   ?view=checklist|subscription|library|inbox      the sidebar checklist, Costs' subscription value card, the Library
//                                                   overview strip, the empty Inbox
//   ?theme=dark                                     dark tokens
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { Component, type ReactNode, StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { Page as AttentionPage } from '@/components/attention/Page'
import { SubscriptionValue } from '@/components/costs/SubscriptionValue'
import { LibraryOverview } from '@/components/library/Overview'
import { Checklist } from '@/components/onboarding/Checklist'
import { LinkPhoneDialog, useLinkPhone } from '@/components/onboarding/LinkPhoneDialog'
import { Wizard } from '@/components/onboarding/Wizard'
import { SidebarProvider } from '@/components/ui/sidebar'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import { index } from '@/lib/costs/cube'
import { LIMITS_KEY } from '@/lib/costs/queries'
import { WIZARD_STEPS, type WizardStep, useOnboarding } from '@/lib/onboarding/state'
import { queryKeys } from '@/lib/rpc'
import { type Api, METHODS, SUBSCRIPTIONS } from '@/rpc/bindings/client'
import type { ConfigFlagKey } from '@/rpc/bindings/method.rpc_2dcompose.input'
import type { ModelInfo } from '@/rpc/bindings/method.agent.output'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'
import { costsFixture, limitsFixture } from './costs-fixture'
import { GROUPS } from '@/lib/nav'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
const view = query.get('view')
const dialog = query.get('dialog')
const step = (WIZARD_STEPS as readonly string[]).includes(query.get('step') ?? '') ? (query.get('step') as WizardStep) : 'welcome'
const now = Date.now()

const model = (id: string, name: string, context: number, efforts: string[] = [], local = false): ModelInfo => ({ id, name, provider: id.split('/')[0]!, context, reasoning: efforts.length > 0, image: !local, efforts, budget_min: null, budget_max: null, local })
const CATALOG: ModelInfo[] =
  query.get('models') === 'empty'
    ? []
    : [
        model('anthropic/claude-opus-4-5', 'Claude Opus 4.5', 200_000, ['low', 'medium', 'high']),
        model('anthropic/claude-sonnet-4-5', 'Claude Sonnet 4.5', 1_000_000, ['low', 'medium', 'high']),
        model('anthropic/claude-haiku-4-5', 'Claude Haiku 4.5', 200_000),
        model('openai/gpt-5', 'GPT-5', 400_000, ['minimal', 'low', 'medium', 'high']),
        model('openai/gpt-5-mini', 'GPT-5 mini', 400_000),
        model('ollama/qwen3:30b', 'qwen3 30b', 32_768, [], true),
      ]
const info = { name: 'fixture', version: '2.27.4', data_dir: '~/.theuth', default_model: CATALOG[1]?.id ?? 'anthropic/claude-sonnet-4-5', default_effort: null }
const flags: Record<string, boolean> = { 'onboarding.completed': view === 'checklist' || view === 'library' }
const flag = (key: ConfigFlagKey) => ({ key, enabled: flags[key] ?? false, running: true, restart_required: false })
const ok = <T,>(v: T) => Promise.resolve(v)
const base: Record<string, (...a: never[]) => Promise<unknown>> = {
  info: () => ok(info),
  list_models: () => ok(CATALOG),
  list_logins: () =>
    ok([
      { name: 'anthropic', logged_in: view === 'subscription', account: view === 'subscription' ? 'you@example.com' : null, expires_at: null },
      { name: 'openai', logged_in: false, account: null, expires_at: null },
      { name: 'xai', logged_in: false, account: null, expires_at: null },
      { name: 'cursor', logged_in: false, account: null, expires_at: null },
    ]),
  list_providers: () => ok([]),
  // A sign-in stays waiting: the browser half has no fixture.
  login: (ns: never) => ok(`https://auth.example.com/${ns as string}/authorize?fixture`),
  local_servers_probe: () =>
    ok([
      { runtime: 'ollama', base_url: 'http://127.0.0.1:11434/v1/', reachable: true, models: 3, error: null, added_as: null },
      { runtime: 'lmstudio', base_url: 'http://127.0.0.1:1234/v1/', reachable: false, models: 0, error: 'connection refused', added_as: null },
    ]),
  add_provider: (name: never) => ok({ name, models: 3 }),
  set_secret: (name: never) => ok({ name }),
  set_model: (_s: never, id: never) => {
    info.default_model = id
    active().connection.setState({ info: { ...info } as never })
    return ok(null)
  },
  set_effort: () => ok(null),
  config_flag_get: (key: never) => ok(flag(key)),
  config_flag_set: (key: never, on: never) => {
    flags[key as string] = on
    return ok(flag(key))
  },
  device_pair_start: () => ok({ string: 'theuth-pair:v1:fixture-relay.theuth.io:9f2c71d0e4a84b3c:5d1e9a', expires_ms: Date.now() + 5 * 60_000, pending_id: 'p-fixture' }),
  devices_list: () => ok([]),
  list_sessions: () => ok([]),
  list_skills: () => ok(Array.from({ length: 9 }, (_, i) => ({ name: `skill-${i}` }))),
  attention_list: () => ok([]),
  cost_cube: () => ok(costsFixture(now)),
  usage_limits: () => ok(limitsFixture(now)),
  slack_status: () => ok({ connected: false }),
  telegram_status: () => ok({ connected: false }),
  signal_status: () => ok({ registered: false }),
}
const api = new Proxy(base as unknown as Api, {
  get: (t, name: string) =>
    (t as unknown as typeof base)[name] ??
    (async () => {
      throw new Error(`fixture: ${name} not answered`)
    }),
})
// A discovery document that offers every method and every topic the Library tabs ask for: capability-gated queries
// run against `base`, and `supports()` (the Library strip's tab filter) answers yes.
const topics = GROUPS.library.flatMap((t) => t.required.topics ?? [])
const doc = {
  methods: METHODS.map((name) => ({ name })),
  'x-theuth': {
    subscriptions: Object.entries(SUBSCRIPTIONS).map(([subscribe, s]) => ({ subscribe, notification: s.notification, unsubscribe: s.unsubscribe })),
    events: topics.map((key) => ({ topic: key.slice(0, key.lastIndexOf('@')), version: Number(key.slice(key.lastIndexOf('@') + 1)) })),
  },
}
active().connection.setState({ api, client: {} as RpcClient, info: info as never, discovery: { phase: 'ready', capabilities: null, document: doc as never, unavailable: new Set() } })
const qc = active().qc
qc.setQueryData(LIMITS_KEY, limitsFixture(now))
// The Library strip reads these from the caches their pages fill.
qc.setQueryData(queryKeys.artifacts, Array.from({ length: 14 }, () => ({})))
qc.setQueryData(queryKeys.knowledgeList, Array.from({ length: 37 }, () => ({})))

// The guides (checklist, Library strip) show after the wizard, finished or skipped, and while not hidden.
useOnboarding.setState({ completed: flags['onboarding.completed'] ?? false, checklistDismissed: false })
if (dialog === 'phone') useLinkPhone.setState({ open: true })
else if (!view) useOnboarding.setState({ wizardOpen: true, step })
// The sign-in has no browser half here: press Sign in once and it stays waiting.
if (query.get('login') === 'pending')
  void (async () => {
    for (let i = 0; i < 50; i++) {
      const button = document.querySelector<HTMLButtonElement>('button[aria-label="Sign in: Claude Pro / Max"]')
      if (button) return button.click()
      await new Promise((r) => setTimeout(r, 100))
    }
  })()

/** A view's failure shows its error instead of a blank page. */
class Guard extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null }
  static getDerivedStateFromError(error: Error) {
    return { error }
  }
  render() {
    return this.state.error ? <pre className="p-4 text-sm text-danger">{String(this.state.error.stack)}</pre> : this.props.children
  }
}

function View() {
  switch (view) {
    case 'checklist':
      return (
        <SidebarProvider open>
          <aside className="flex h-dvh w-72 flex-col justify-end border-r border-border-subtle bg-sidebar p-2">
            <Checklist />
          </aside>
        </SidebarProvider>
      )
    case 'subscription':
      return (
        <div className="mx-auto max-w-3xl p-6">
          <SubscriptionValue ix={index(costsFixture(now))} now={now} />
        </div>
      )
    case 'library':
      return (
        <SidebarProvider open>
          <div className="w-full">
            <LibraryOverview />
          </div>
        </SidebarProvider>
      )
    case 'inbox':
      return (
        <SidebarProvider open>
          <AttentionPage />
        </SidebarProvider>
      )
    default:
      return (
        <>
          <p className="p-6 font-mono text-xs text-fg-tertiary">215 onboarding fixture</p>
          <Wizard />
          <LinkPhoneDialog />
        </>
      )
  }
}

function Fixture() {
  return (
    <TooltipProvider>
      <Guard>
        <View />
      </Guard>
      <Toaster />
    </TooltipProvider>
  )
}

// The Library strip shows on a Library tab's list page; the Inbox's detail outlet renders nothing (no item open).
const router = createRouter({
  routeTree: createRootRoute({ component: Fixture, notFoundComponent: () => null }),
  history: createMemoryHistory({ initialEntries: [view === 'library' ? '/notes' : view === 'inbox' ? '/attention' : '/'] }),
})

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
