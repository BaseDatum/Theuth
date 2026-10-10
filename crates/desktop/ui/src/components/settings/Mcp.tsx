// 158 W5 owns this file (stub from C158). Settings › MCP server (158 §12.6): the on/off switch, port, token
// with Rotate, "Copy MCP config" per client, the audit tail. Hidden from the tab list until W5 flips `ready`.

import { Empty } from '@/components/settings/shared'

/** `SettingsDialog` drops this tab while `false`; W5 flips it with the real panel. */
export const ready = false

export function Mcp() {
  return <Empty>MCP server settings arrive with 158 W5.</Empty>
}
