import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/hud/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { Hud } from '@/components/hud/Hud'

/** The THEUTH mark's overview (70 HUD). New chat lives at `/new`. */
export const Route = createFileRoute('/')({ component: withCapability(Hud, requirements) })
