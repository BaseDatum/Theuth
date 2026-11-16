import { Badge } from '@/components/ui/badge'
import { type AgentFile, type AgentSummary, isIsolated } from '@/lib/agent-memory'

export function MemoryBadge({ agent }: { agent: AgentFile | AgentSummary }) {
  return isIsolated(agent) ? <Badge variant="outline" className="shrink-0 text-fg-secondary">Isolated memory</Badge> : null
}
