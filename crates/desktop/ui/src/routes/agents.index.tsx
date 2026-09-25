import { createFileRoute } from '@tanstack/react-router'
import { AgentsView } from '@/components/agents/AgentsView'

export const Route = createFileRoute('/agents/')({ component: AgentsView })
