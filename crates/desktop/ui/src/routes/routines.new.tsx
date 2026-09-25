import { createFileRoute } from '@tanstack/react-router'
import { RoutineDetail } from '@/components/routines/RoutineDetail'

export const Route = createFileRoute('/routines/new')({ component: () => <RoutineDetail name={null} section="overview" /> })
