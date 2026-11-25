import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/whiteboard/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { WhiteboardsList } from '@/components/whiteboard/WhiteboardsList'

/** `/whiteboards` (218): the whiteboards list. */
export const Route = createFileRoute('/whiteboards')({ component: withCapability(WhiteboardsList, requirements) })
