import { createFileRoute } from '@tanstack/react-router'
import { Page } from '@/components/attention/Page'

/** The attention inbox as master–detail (101 D6): the rail here, the selected item's screen in the outlet. The sidebar row's badge is its count (70 HUD). */
export const Route = createFileRoute('/attention')({ component: Page })
