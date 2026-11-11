import { MousePointer2, Plug, Search as SearchIcon, Server, Wrench } from 'lucide-react'
import { McpServersTab } from '@/components/library/Tools'
import { Computer } from '@/components/settings/Computer'
import { Mcp } from '@/components/settings/Mcp'
import { Search } from '@/components/settings/Search'
import type { SettingsCategory } from '@/components/settings/types'

// 188: Tools (docs/design/188-settings-full-page.md). MCP first (Sky, 2026-11-11): the servers theuth uses, moved here
// from Agents › Tools; its id is `tools`, so `/settings/tools` (and `/tools`) land on it.
export const tools: SettingsCategory = {
  id: 'tools',
  label: 'Tools',
  icon: Wrench,
  tabs: [
    { id: 'tools', label: 'MCP', icon: Server, Component: McpServersTab },
    { id: 'search', label: 'Web search', icon: SearchIcon, Component: Search },
    { id: 'mcp', label: 'Serve over MCP', icon: Plug, Component: Mcp },
    { id: 'computer', label: 'Computer use', icon: MousePointer2, Component: Computer },
  ],
}
