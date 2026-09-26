import { createFileRoute } from '@tanstack/react-router'
import { AttentionView } from '@/components/hud/AttentionView'

/** The attention inbox on its own; the sidebar row's badge is its count (70 HUD). */
export const Route = createFileRoute('/attention')({ component: AttentionView })
