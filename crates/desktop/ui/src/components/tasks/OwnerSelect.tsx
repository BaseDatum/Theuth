import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'

/** Owner or reviewer: "you" plus every roster agent. `value` is the text form (`user` | `agent:<name>`). */
export function OwnerSelect({
  id,
  value,
  agents,
  onChange,
  className,
}: {
  id?: string
  value: string
  agents: readonly AgentSummary[]
  onChange(v: string): void
  className?: string
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger id={id} size="sm" className={className ?? 'w-48'}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="user">you</SelectItem>
        {agents.length > 0 && <SelectSeparator />}
        {agents.map((a) => (
          <SelectItem key={a.file.name} value={`agent:${a.file.name}`}>
            {a.file.title}
            <span className="text-fg-tertiary">{a.file.name}</span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}
