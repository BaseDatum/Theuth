// 216 §10.1: the plain-words lines of what a plugin may do, each with the risk it maps to — the install card's lines
// (`InstallCard.lines`, written by the host) and Settings › Plugins' "Granted" list (`grantedLines`, from `Grants`).
import { cn } from '@/lib/utils'
import type { GrantLine, Grants, Risk } from '@/rpc/bindings/method.plugins.output'

/** A risk in the words of Settings › Tools. */
export const RISK_WORD: Record<Risk, string> = { read_only: 'read-only', mutating: 'mutating', destructive: 'destructive' }

const RISK_TONE: Record<Risk, string> = {
  read_only: 'text-fg-tertiary',
  mutating: 'text-warning',
  destructive: 'text-danger',
}

/** Control, line-separator, zero-width and bidirectional characters: drawn verbatim they can reorder or hide text. */
const UNSAFE = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u061c\u200b-\u200f\u2028-\u202e\u2060-\u2069\ufeff]/g

/**
 * A plugin-supplied string as the owner should read it on an approval card or in Settings (216 §10.1): each control,
 * BiDi or invisible character shown as its `\uXXXX` escape, so it cannot visually spoof a command, path or line.
 * Tab and newline stay.
 */
export const shown = (s: string): string => s.replace(UNSAFE, (c) => `\\u${c.charCodeAt(0).toString(16).toUpperCase().padStart(4, '0')}`)

/** One row per line: the text, a "new" chip on a widen card's added lines, the risk word on the right. */
export function GrantLines({ lines, className }: { lines: readonly GrantLine[]; className?: string }) {
  return (
    <ul className={cn('flex flex-col', className)}>
      {lines.map((line, i) => (
        <li key={`${line.key}-${i}`} data-grant={line.key} data-new={line.new || undefined} className="flex items-baseline gap-3 border-b border-border-subtle py-2 text-sm last:border-0">
          <span className="min-w-0 flex-1 wrap-break-word">
            {line.new && <span className="mr-2 rounded-full bg-fg-accent/10 px-1.5 py-px text-[11px] font-medium text-fg-accent">new</span>}
            {shown(line.text)}
          </span>
          <span className={cn('shrink-0 text-[13px]', RISK_TONE[line.risk])}>{RISK_WORD[line.risk]}</span>
        </li>
      ))}
    </ul>
  )
}

const line = (key: GrantLine['key'], text: string, risk: Risk): GrantLine => ({ key, text, risk, new: false })

/**
 * What a verified grant holds, as card-like lines (216 contract §12.6: read-only in v1). The host words the install
 * card itself; these are the same entries in short form: run, secrets, then the order of §10.1's table.
 */
export function grantedLines(g: Grants): GrantLine[] {
  const call = g.capabilities.call
  return [
    ...(g.run.length > 0 ? [line('run', `Runs ${g.run.join(' ')}${g.cwd ? ` from ${g.cwd}` : ''}`, 'destructive')] : []),
    ...g.secrets.map((s) => line('secret', `Gets the secret ${s} in its environment`, 'destructive')),
    ...g.subscribe.map((s) => line('subscribe', `Reads ${s}`, 'read_only')),
    ...g.emits.map((e) => line('emit', `Publishes ${e}`, 'read_only')),
    ...g.placements.map((p) => line('placement', `Shows the placement ${p}`, 'read_only')),
    ...g.tools.map((t) => line('tool', `Offers the model the tool ${t.name}`, t.risk)),
    ...(call ? [line('call', `May call ${call.tools.join(', ')} in its own background session`, call.max_risk)] : []),
    ...(g.capabilities.input ? [line('input', 'May send messages into your chats as you', 'destructive')] : []),
    ...(g.capabilities.notify ? [line('notify', 'May add inbox items and notifications', 'read_only')] : []),
  ]
}
