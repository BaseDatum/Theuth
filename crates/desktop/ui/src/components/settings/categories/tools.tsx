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
    { required: {"methods":["add_mcp_repo","add_mcp_server","list_mcp","mcp_login","mcp_logout","mcp_status","mesh_status","remove_mcp_server","set_mcp_gate","set_mcp_hidden","set_mcp_home","sync_mcp"],"topics":[]}, id: 'tools', label: 'MCP', icon: Server, Component: McpServersTab },
    { required: {"methods":["search_provider_test","search_settings_get","search_settings_set","search_status","search_test","set_secret"],"topics":[]}, id: 'search', label: 'Web search', icon: SearchIcon, Component: Search },
    { required: {"methods":["mcp_server_audit","mcp_server_get","mcp_server_rotate_token","mcp_server_set"],"topics":[]}, id: 'mcp', label: 'Serve over MCP', icon: Plug, Component: Mcp },
    { required: {"methods":["computer_permissions","config_flag_get","config_flag_set"],"topics":[]}, id: 'computer', label: 'Computer use', icon: MousePointer2, Component: Computer },
  ],
}
