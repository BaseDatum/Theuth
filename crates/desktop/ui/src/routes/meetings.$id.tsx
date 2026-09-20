import { createFileRoute } from '@tanstack/react-router'
import { ControlCenter } from '@/components/meeting/ControlCenter'

/** 53 §4.3: the control center while recording; the manifest after. */
export const Route = createFileRoute('/meetings/$id')({ component: Page })

function Page() {
  const { id } = Route.useParams()
  return <ControlCenter id={id} />
}
