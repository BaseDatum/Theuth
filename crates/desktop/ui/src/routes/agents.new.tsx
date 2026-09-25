import { createFileRoute } from '@tanstack/react-router'
import { HireWizard } from '@/components/agents/HireWizard'

export const Route = createFileRoute('/agents/new')({ component: HireWizard })
