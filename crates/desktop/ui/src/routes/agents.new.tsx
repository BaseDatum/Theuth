import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/agents/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { HireWizard } from '@/components/agents/HireWizard'

export const Route = createFileRoute('/agents/new')({ component: withCapability(HireWizard, requirements) })
