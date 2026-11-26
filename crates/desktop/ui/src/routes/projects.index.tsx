import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/projects/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { ProjectsView } from '@/components/projects/ProjectsView'

/** `/projects` (223): every project, rows and implicit labels. */
export const Route = createFileRoute('/projects/')({ component: withCapability(ProjectsView, requirements) })
