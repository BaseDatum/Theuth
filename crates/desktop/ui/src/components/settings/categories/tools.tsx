import { MousePointer2, Plug, Search as SearchIcon, Wrench } from 'lucide-react'
import { Computer } from '@/components/settings/Computer'
import { Mcp } from '@/components/settings/Mcp'
import { Search } from '@/components/settings/Search'
import type { SettingsCategory } from '@/components/settings/types'

// 188: Tools (docs/design/188-settings-full-page.md).
export const tools: SettingsCategory = {
  id: 'tools',
  label: 'Tools',
  icon: Wrench,
  tabs: [
    { id: 'search', label: 'Web search', icon: SearchIcon, Component: Search },
    { id: 'mcp', label: 'Serve over MCP', icon: Plug, Component: Mcp },
    { id: 'computer', label: 'Computer use', icon: MousePointer2, Component: Computer },
  ],
}
