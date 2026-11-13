// 158 §12.6: the MCP client config blocks Settings › MCP copies, and the RPC call shapes behind its controls.
// Claude Code and Cursor read the same `mcpServers` JSON (theuth's own client parses it too, `crates/tools/src/mcp.rs`);
// Codex reads `~/.codex/config.toml`; `stdio` points a client at `t mcp` against the running engine (§12.3).

import { stringify as toml } from 'smol-toml'
import type { Api } from '@/rpc/bindings/client'
import type { McpServerStatus } from "@/rpc/bindings/method.mcp_2dserver.output"

export const MCP_CLIENTS = ['claude', 'codex', 'cursor', 'stdio'] as const
export type McpClient = (typeof MCP_CLIENTS)[number]

export const MCP_CLIENT_LABEL: Record<McpClient, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
  cursor: 'Cursor',
  stdio: 'stdio (t mcp)',
}

/** The lawful line that closes the panel (§12.6 as amended by A2: tools are read and write). */
export const MCP_SHARED_LINE =
  'What is shared: meetings, notes, knowledge, notebooks, sessions, tasks, artifacts — read and write. Private sessions are never shared.'

/** Text the Copy button puts on the clipboard for `client`. A missing token is written as `<token>` so the block stays valid. */
export function mcpConfig(client: McpClient, status: Pick<McpServerStatus, 'url' | 'token' | 'binary'>, dataDir: string): string {
  const url = status.url
  const auth = `Bearer ${status.token ?? '<token>'}`
  switch (client) {
    case 'claude':
    case 'cursor':
      return JSON.stringify({ mcpServers: { theuth: { type: 'http', url, headers: { Authorization: auth } } } }, null, 2)
    case 'codex':
      return toml({ mcp_servers: { theuth: { url, http_headers: { Authorization: auth } } } })
    case 'stdio':
      // The engine's own executable (`current_exe`): PATH may hold an older `t`. `t` only when the OS could not tell.
      return JSON.stringify({ mcpServers: { theuth: { command: status.binary ?? 't', args: ['--data-dir', dataDir, 'mcp'] } } }, null, 2)
  }
}

/** The three RPC calls behind the panel's controls, in one place so a test can exercise them without a DOM. */
export const mcpActions = (api: Pick<Api, 'mcp_server_set' | 'mcp_server_rotate_token'>) => ({
  setEnabled: (status: McpServerStatus, enabled: boolean) => api.mcp_server_set({ enabled, port: status.port }),
  setPort: (status: McpServerStatus, port: number) => api.mcp_server_set({ enabled: status.enabled, port }),
  rotate: () => api.mcp_server_rotate_token(),
})
