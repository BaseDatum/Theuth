import { createFileRoute } from '@tanstack/react-router'
import { Viewer } from '@/components/artifacts/Viewer'

/** `callId` is the `show` call that made the artifact (35 §7.3). */
export const Route = createFileRoute('/artifacts/$callId')({ component: Viewer })
