import { createFileRoute } from '@tanstack/react-router'
import { Viewer } from '@/components/artifacts/Viewer'
import { useMarkShown } from '@/components/hud/queries'

/** `callId` is the `show` call that made the artifact (35 §7.3). 104 A2: opening it here is seeing it — its inbox item goes. */
function LibraryArtifact() {
  const { callId } = Route.useParams()
  useMarkShown(callId)
  return <Viewer />
}

export const Route = createFileRoute('/artifacts/$callId')({ component: LibraryArtifact })
