import { createFileRoute } from '@tanstack/react-router'
import { NodeTerminal, nodeTerminalRequirements } from '@/components/cloud/NodeTerminal'
import { withCapability } from '@/rpc/capabilities'

/** One standalone node login shell, full window (`lib/popout.ts` kind `node-terminal`). Under the `Popped` root when opened with `?window=`. */
export const Route = createFileRoute('/terminal/$engine/$shell')({
  validateSearch: (search: Record<string, unknown>): { name?: string } => ({
    ...(typeof search.name === 'string' && search.name.length > 0 ? { name: search.name } : {}),
  }),
  component: withCapability(NodeTerminalRoute, nodeTerminalRequirements),
})

function NodeTerminalRoute() {
  const { engine, shell } = Route.useParams()
  const { name } = Route.useSearch()
  return <NodeTerminal engine={engine} shell={shell} name={name} />
}
