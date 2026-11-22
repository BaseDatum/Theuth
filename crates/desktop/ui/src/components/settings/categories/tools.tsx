import { Blocks, Globe, MousePointer2, Plug, Search as SearchIcon, Server, Wrench } from 'lucide-react'
import { BuiltinsTab } from '@/components/library/Builtins'
import { McpServersTab } from '@/components/library/Tools'
import { BrowserProfile } from '@/components/settings/BrowserProfile'
import { Computer } from '@/components/settings/Computer'
import { Mcp } from '@/components/settings/Mcp'
import { Search } from '@/components/settings/Search'
import type { SettingsCategory } from '@/components/settings/types'
import { nativeWebViews } from '@/lib/web-view-native'

// 188: Tools (docs/design/188-settings-full-page.md). MCP first (Sky, 2026-11-11): the servers theuth uses, moved here
// from Agents › Tools; its id is `tools`, so `/settings/tools` (and `/tools`) land on it. 206: Builtins, the harness's own
// tools by category, right after it. 209 §6: Browser, the shared browser profile of the web views and agent browsers, where this shell hosts web views.
export const tools: SettingsCategory = { 
  id: 'tools',
  label: 'Tools',
  icon: Wrench,
  tabs: [
    { required: {"methods":["add_mcp_repo","add_mcp_server","list_mcp","mcp_login","mcp_logout","mcp_status","mesh_status","remove_mcp_server","set_mcp_gate","set_mcp_hidden","set_mcp_home","sync_mcp"],"topics":[]}, id: 'tools', label: 'MCP', icon: Server, Component: McpServersTab },
    { required: {"methods":["list_builtins","set_builtins_disabled","set_mcp_hidden"],"topics":[]}, id: 'builtins', label: 'Builtins', icon: Blocks, Component: BuiltinsTab },
    { required: {"methods":["search_provider_test","search_settings_get","search_settings_set","search_status","search_test","set_secret"],"topics":[]}, id: 'search', label: 'Web search', icon: SearchIcon, Component: Search },
    { required: {"methods":["mcp_server_audit","mcp_server_get","mcp_server_rotate_token","mcp_server_set"],"topics":[]}, id: 'mcp', label: 'Serve over MCP', icon: Plug, Component: Mcp },
    { required: {"methods":["computer_permissions","config_flag_get","config_flag_set"],"topics":[]}, id: 'computer', label: 'Computer use', icon: MousePointer2, Component: Computer },
    { required: {"methods":["web_profile_clear","web_profile_forget","web_profile_sites"],"topics":["browser.web-profile-changed@1"]}, id: 'browser', label: 'Browser', icon: Globe, Component: BrowserProfile, visible: nativeWebViews },
  ],
}
