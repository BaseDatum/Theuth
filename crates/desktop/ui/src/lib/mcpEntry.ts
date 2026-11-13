/** `/mcp add` argument that means "clone a repo": a `.git` URL or a known git host (`tui/model.rs::git_url`). */
export function gitUrl(s: string): boolean {
  return s.endsWith('.git') || ['github.com/', 'gitlab.com/', 'codeberg.org/'].some((h) => s.includes(h))
}

type JsonObject = Record<string, unknown>

function isObject(v: unknown): v is JsonObject {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

/**
 * Parse one MCP server from pasted JSON (`tui/model.rs::mcp_entry`): a server
 * object with a `name` key (stripped), `{ "<name>": {...} }`, or
 * `{ "mcpServers": { "<name>": {...} } }`. Throws with the TUI's messages.
 */
export function mcpEntry(text: string): { name: string; entry: unknown } {
  const v: unknown = JSON.parse(text)
  if (!isObject(v)) throw new Error('expected a JSON object')
  const servers = v.mcpServers
  const obj = isObject(servers) ? servers : v
  if ('command' in obj || 'url' in obj) {
    const { name, ...entry } = obj
    if (typeof name !== 'string') throw new Error('server object needs a "name" field')
    return { name, entry }
  }
  const entries = Object.entries(obj)
  const first = entries[0]
  if (entries.length === 1 && first && isObject(first[1])) return { name: first[0], entry: first[1] }
  throw new Error('expected exactly one server: {"<name>": {"command": ...}}')
}
