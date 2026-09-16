import { Outlet, createRootRoute, useNavigate, useParams } from '@tanstack/react-router'
import { type CSSProperties, useEffect, useState } from 'react'
import { settings } from '@/components/chat/slash'
import { AppSidebar } from '@/components/sidebar/AppSidebar'
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar'
import { useHotkeys } from '@/lib/hotkeys'

export const Route = createRootRoute({ component: Root })

const WIDTH = { '--sidebar-width': '260px' } as CSSProperties

/** The shell: 260 px sidebar (⇧⌘S) beside the routed column. Routes render only once connected, so `useApi` is safe below. */
function Root() {
  const { session } = useParams({ strict: false })
  const navigate = useNavigate()
  const [open, setOpen] = useState(true)
  useHotkeys()

  useEffect(() => {
    const toggle = () => setOpen((o) => !o)
    const fresh = () => void navigate({ to: '/' })
    const shortcuts = () => void settings(navigate, 'shortcuts', session)
    window.addEventListener('wth:toggle-sidebar', toggle)
    window.addEventListener('wth:new-chat', fresh)
    window.addEventListener('wth:shortcuts', shortcuts)
    return () => {
      window.removeEventListener('wth:toggle-sidebar', toggle)
      window.removeEventListener('wth:new-chat', fresh)
      window.removeEventListener('wth:shortcuts', shortcuts)
    }
  }, [navigate, session])

  return (
    <SidebarProvider open={open} onOpenChange={setOpen} style={WIDTH} className="h-band min-h-0">
      <AppSidebar session={session} />
      <SidebarInset className="min-h-0 overflow-hidden pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]">
        <Outlet />
      </SidebarInset>
    </SidebarProvider>
  )
}
