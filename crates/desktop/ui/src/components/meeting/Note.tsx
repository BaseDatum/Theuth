import { useMemo } from 'react'
import { Live } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { cn } from '@/lib/utils'

/**
 * The meeting's note (59 §7.4) in BlockNote: beside the live transcript while recording, the summary view once
 * the summary turn appended its sections. The same document, so what the user typed during the call is what the
 * agent regrouped — hover attribution tells the two apart.
 */
export function MeetingNote({ doc, className }: { doc: string; className?: string }) {
  const target = useMemo(() => ({ doc }), [doc])
  const live = useDocument(target)
  return (
    <div className={cn('flex min-h-0 min-w-0 flex-1 flex-col rounded-xl border border-border-subtle', className)}>
      <Live {...live} />
    </div>
  )
}
