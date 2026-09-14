import { Link, useNavigate } from '@tanstack/react-router'
import { BookOpen, PanelLeft, Search, Settings, SquarePen } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from '@/components/ui/sidebar'
import { settings } from '@/components/chat/slash'
import { SearchDialog } from './SearchDialog'
import { SessionList } from './SessionList'

/** ChatGPT's left rail: New chat, Search chats, Knowledge, the chat list, Settings. */
export function AppSidebar({ session }: { session: string | undefined }) {
  const navigate = useNavigate()
  const { toggleSidebar } = useSidebar()
  const scrollRef = useRef<HTMLDivElement>(null)
  const [search, setSearch] = useState(false)

  useEffect(() => {
    const open = () => setSearch(true)
    window.addEventListener('wth:search', open)
    return () => window.removeEventListener('wth:search', open)
  }, [])

  return (
    <Sidebar collapsible="offcanvas" className="border-r-0">
      <SidebarHeader data-tauri-drag-region className="h-[52px] flex-row items-center justify-end pl-20">
        <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Close sidebar" onClick={toggleSidebar}>
          <PanelLeft className="size-5" />
        </Button>
      </SidebarHeader>
      <SidebarContent ref={scrollRef} className="gap-0 overflow-y-auto">
        <SidebarMenu className="px-1.5">
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link to="/">
                <SquarePen /> New chat
                <span className="ml-auto text-xs text-fg-tertiary">⇧⌘O</span>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton className="h-9 rounded-[10px] px-2.5" onClick={() => setSearch(true)}>
              <Search /> Search chats
              <span className="ml-auto text-xs text-fg-tertiary">⌘K</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link to="/knowledge">
                <BookOpen /> Knowledge
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
        <SessionList scrollRef={scrollRef} active={session} />
      </SidebarContent>
      <SidebarFooter className="px-1.5">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton className="h-9 rounded-[10px] px-2.5" onClick={() => void settings(navigate, 'general', session)}>
              <Settings /> Settings
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SearchDialog open={search} onOpenChange={setSearch} />
    </Sidebar>
  )
}
