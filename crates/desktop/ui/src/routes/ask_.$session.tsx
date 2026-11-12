import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/notebooks/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { AskView } from '@/components/notebooks/AskView'

/** `/ask/$session` (144 §9, wave 2): one ask-across-notebooks run — the plan, the branches, the answer — over the `kind: ask` session's chat. */
export const Route = createFileRoute('/ask_/$session')({ component: withCapability(AskView, requirements) })
