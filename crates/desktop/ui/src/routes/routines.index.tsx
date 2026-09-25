import { createFileRoute } from '@tanstack/react-router'
import { RoutinesView } from '@/components/routines/RoutinesView'

export const Route = createFileRoute('/routines/')({ component: RoutinesView })
