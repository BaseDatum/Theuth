// 187: an MCP server's home machine and its state on this one, as each server's row on Settings › Tools › MCP shows them (188 D2).
// Pure: the mesh (for current machine names) comes in, nothing is fetched here.

import { type Engine, UNNAMED_MACHINE } from '@/components/tasks/home'
import type { McpHomeAction } from '@/rpc/bindings/McpHomeAction'
import type { McpServer } from '@/rpc/bindings/McpServer'

type Server = Pick<McpServer, 'home' | 'home_name' | 'home_here' | 'everywhere' | 'state' | 'reason'>

/** The home's name: the mesh's current one, else the name it was stamped with; null while nobody claimed it. */
export function homeName(s: Server, engines: readonly Engine[]): string | null {
  if (s.home_here) return engines.find((e) => e.here)?.name ?? 'this machine'
  if (!s.home) return null
  return engines.find((e) => e.id === s.home)?.name ?? s.home_name ?? UNNAMED_MACHINE
}

/** The row's chip: `on 333`, or that no machine claimed it yet. */
export const homeChip = (s: Server, engines: readonly Engine[]): string => {
  const name = homeName(s, engines)
  return name ? `on ${name}` : 'home not claimed yet'
}

/** The state on this machine, in words. */
export function stateText(s: Server, engines: readonly Engine[]): string {
  switch (s.state) {
    case 'running':
      return 'running'
    case 'connecting':
      return 'starting…'
    case 'elsewhere':
      return `not running here (home is ${homeName(s, engines) ?? 'not claimed yet'})`
    case 'off_here':
      return 'turned off here'
    case 'off':
      return 'off'
    case 'unavailable_here':
      return 'not installed here'
    case 'error':
      return 'error'
  }
}

export type HomeAction = { action: McpHomeAction; label: string; pressed?: boolean }

/**
 * What a row offers on this machine: the Run everywhere toggle (pressed while on, then also Only on <home>), Move home
 * here, and Turn off here (only where it would run) / Turn on here. Alone in the mesh only Move home here, and only for
 * a home on a machine the mesh does not know (an unpaired one, a reset identity).
 */
export function homeActions(s: Server, engines: readonly Engine[]): HomeAction[] {
  const home = homeName(s, engines)
  const move: HomeAction = { action: 'move_here', label: 'Move home here' }
  if (engines.length <= 1) return !s.home_here && !engines.some((e) => e.id === s.home) ? [move] : []
  const out: HomeAction[] = [{ action: s.everywhere ? 'only_home' : 'run_everywhere', label: 'Run everywhere', pressed: s.everywhere }]
  if (s.everywhere) out.push({ action: 'only_home', label: home ? `Only on ${home}` : 'Only at home' })
  if (!s.home_here) out.push(move)
  if (s.state === 'off_here') out.push({ action: 'on_here', label: 'Turn on here' })
  else if (['running', 'connecting', 'unavailable_here', 'error'].includes(s.state)) out.push({ action: 'off_here', label: 'Turn off here' })
  return out
}
