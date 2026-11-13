import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/routines/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { RoutinesView } from '@/components/routines/RoutinesView'

export const Route = createFileRoute('/routines/')({ component: withCapability(RoutinesView, requirements) })
