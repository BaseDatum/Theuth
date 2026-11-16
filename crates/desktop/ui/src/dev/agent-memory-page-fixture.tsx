import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet } from '@tanstack/react-router'
import { createRoot } from 'react-dom/client'
import { AgentDetail, type Tab } from '@/components/agents/AgentDetail'
import { AgentsView } from '@/components/agents/AgentsView'
import { HireWizard } from '@/components/agents/HireWizard'
import { DRAFT_KEY, emptyDraft } from '@/components/agents/wizard'
import { SidebarProvider } from '@/components/ui/sidebar'
import { TooltipProvider } from '@/components/ui/tooltip'
import { active } from '@/store/engine-scope'
import { memoryFixture } from './agent-memory-fixture'

// Dev-only: /src/dev/agent-memory-fixture.html?page=agents|memory|runtime|hire&state=empty|error|refuse&theme=dark
const query = new URLSearchParams(location.search)
const fixture = memoryFixture(query.get('state') ?? 'notes')
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
localStorage.setItem('theuth.agents-view', 'list')
localStorage.setItem(DRAFT_KEY, JSON.stringify({ step: 'runtime', draft: { ...emptyDraft(), name: 'new-research', title: 'Researcher', body: 'Research charter.', isolated_memory: true } }))
active().connection.setState({ api: fixture.api, state: 'open', discovery: { ...active().connection.getState().discovery, phase: 'legacy' } })
const root = createRootRoute({ component: () => <TooltipProvider><SidebarProvider><div className="flex h-dvh min-w-0 flex-1 flex-col bg-background"><Outlet /></div></SidebarProvider></TooltipProvider> })
const agents = createRoute({ getParentRoute: () => root, path: '/agents', component: AgentsView })
const hire = createRoute({ getParentRoute: () => root, path: '/agents/new', component: HireWizard })
const detail = createRoute({ getParentRoute: () => root, path: '/agents/$name/$tab', component: () => { const { name, tab } = detail.useParams(); return <AgentDetail name={name} tab={tab as Tab} /> } })
const page = query.get('page') ?? 'memory'
const path = page === 'agents' ? '/agents' : page === 'hire' ? '/agents/new' : `/agents/${page === 'runtime' ? 'yang' : 'research'}/${page === 'runtime' ? 'runtime' : 'memory'}`
const router = createRouter({ routeTree: root.addChildren([agents, hire, detail]), history: createMemoryHistory({ initialEntries: [path] }) })
createRoot(document.getElementById('root')!).render(<QueryClientProvider client={active().qc}><RouterProvider router={router} /></QueryClientProvider>)
