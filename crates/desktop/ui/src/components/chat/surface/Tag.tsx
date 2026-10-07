import { Badge } from '@/components/ui/badge'

const BADGE: Record<string, string> = {
  success: 'border-success-border bg-success-bg text-success',
  warning: 'border-warning-border bg-warning-bg text-warning',
}

/** The `Badge` component body, shared with `Tags` and `Kanban` cards. */
export function Tag({ text, variant }: { text: string; variant?: string }) {
  const v = variant ?? 'default'
  return v in BADGE ? (
    <Badge variant="outline" className={BADGE[v]}>
      {text}
    </Badge>
  ) : (
    <Badge variant={v as 'default' | 'secondary' | 'destructive'}>{text}</Badge>
  )
}
