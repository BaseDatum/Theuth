import { PanelLeft } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useSidebarActions } from '@/components/ui/sidebar'

/** The top-left pane header's way to the sidebar (chat, web and tile headers alike). */
export function SidebarToggle() {
  const { toggleSidebar } = useSidebarActions()
  return (
    <Button
      variant="ghost"
      size="icon"
      className="rounded-lg text-fg-secondary"
      aria-label="Open sidebar"
      onClick={toggleSidebar}
    >
      <PanelLeft className="size-5" />
    </Button>
  )
}
