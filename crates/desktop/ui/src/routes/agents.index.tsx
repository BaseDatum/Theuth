import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/agents/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { AgentsView } from '@/components/agents/AgentsView'

export const Route = createFileRoute('/agents/')({ component: withCapability(AgentsView, requirements) })
