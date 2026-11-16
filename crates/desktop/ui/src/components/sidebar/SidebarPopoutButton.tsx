import { AppWindow } from 'lucide-react'
import { SidebarMenuAction } from '@/components/ui/sidebar'
import { canPopout, openPopout, type PopoutTarget } from '@/lib/popout'
import { cn } from '@/lib/utils'

/**
 * A sibling of the row link: hover/focus chrome never navigates, selects or starts its drag. `target` is the window to
 * open, or, on a creation row, the row's own create step that opens the created thing's window itself.
 */
export function SidebarPopoutButton({ target, className }: { target: PopoutTarget | (() => void) | null; className?: string }) {
  if (!target || !canPopout()) return null
  return (
    <SidebarMenuAction
      data-sidebar="popout-action"
      className={cn('opacity-0 group-hover/popout-row:opacity-100 group-focus-within/popout-row:opacity-100 pointer-coarse:hidden', className)}
      aria-label="Open in new window"
      title="Open in new window"
      onPointerDown={(e) => e.stopPropagation()}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        if (typeof target === 'function') target()
        else void openPopout(target)
      }}
    >
      <AppWindow />
    </SidebarMenuAction>
  )
}
