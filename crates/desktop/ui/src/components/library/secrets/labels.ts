// 221: pure helpers the Secrets page, Settings › Approvals and the request card share. No React, no RPC.

import type { ConnInfo, ConnStatus, DestinationView, ExtEntry } from '@/rpc/bindings/method.secrets.output'
import type { Backend } from './types'

/** The four ask directions (D6); `send` is Bitwarden only. */
export type Direction = 'read' | 'write' | 'delete' | 'send'

const BACKEND_LABEL: Record<Backend, string> = { aws: 'AWS', bw: 'Bitwarden' }

/** `AWS` / `Bitwarden`; a backend this build does not know shows its id. */
export const backendLabel = (backend: string): string => BACKEND_LABEL[backend as Backend] ?? backend

/** `secret:<backend>/<conn>:<direction>`, the 219 entry D6 seeds. */
export const alwaysAskKey = (backend: string, conn: string, dir: Direction): string => `secret:${backend}/${conn}:${dir}`

const KEY = /^secret:(aws|bw)\/([a-z0-9-]{1,32}):(read|write|delete|send)$/

/** The parts of a `secret:` always-ask key, or null for anything else (a tool name, `mcp:<server>`). */
export function parseAskKey(key: string): { backend: Backend; conn: string; dir: Direction } | null {
  const m = KEY.exec(key)
  return m ? { backend: m[1] as Backend, conn: m[2] ?? '', dir: m[3] as Direction } : null
}

/** §8: `personal (Bitwarden) · read` for a `secret:` key; null for any other entry. */
export function secretAskLabel(key: string): string | null {
  const k = parseAskKey(key)
  return k && `${k.conn} (${BACKEND_LABEL[k.backend]}) · ${k.dir}`
}

/** Every always-ask key the connections can carry (Approvals' autocomplete, so a removed one can be put back). */
export function askKeysOf(conns: readonly Pick<ConnInfo, 'backend' | 'conn'>[]): string[] {
  return conns.flatMap((c) => (['read', 'write', 'delete', ...(c.backend === 'bw' ? (['send'] as const) : [])] as Direction[]).map((d) => alwaysAskKey(c.backend, c.conn, d)))
}

/** The status dot's word (§9.1). */
export function statusLabel(s: ConnStatus): string {
  switch (s.state) {
    case 'ready':
      return 'ready'
    case 'locked':
      return 'locked'
    case 'logged_out':
      return 'logged out'
    case 'expired':
      return 'expired'
    case 'error':
      return 'error'
    case 'unsupported':
      return 'unsupported'
  }
}

/** The status's second line: the SSO hint, the probe error. */
export function statusDetail(s: ConnStatus): string | null {
  return s.state === 'expired' ? s.detail.hint : s.state === 'error' || s.state === 'unsupported' ? s.detail : null
}

/** `{{secret:<ref>[#key]}}`: what Copy reference puts on the clipboard. Never a value. */
export const placeholderOf = (ref: string, key?: string | null): string => `{{secret:${ref}${key ? `#${key}` : ''}}}`

/** The refs of an entry that carry a value (§9.1 Reveal): AWS the whole secret plus each known JSON key; Bitwarden each field. */
export function valueRefs(entry: Pick<ExtEntry, 'ref' | 'kind' | 'keys'>, backend: string): { label: string; ref: string }[] {
  if (backend === 'aws') {
    if (entry.kind === 'binary') return []
    return [{ label: 'value', ref: entry.ref }, ...entry.keys.map((k) => ({ label: k, ref: `${entry.ref}#${k}` }))]
  }
  return entry.keys.map((k) => ({ label: k, ref: `${entry.ref}#${k}` }))
}

/** §9.2's strip: `Bitwarden · personal › Personal / theuth · new login "GitHub — CI"`, or `AWS · prod (work-admin, eu-west-1) · app/ci/token · JSON · KMS alias/app`. */
export function destinationLine(d: DestinationView): string {
  if (d.backend === 'aws') {
    const parts = [`AWS · ${d.connection} (${d.summary.replace(/ · /g, ', ')})`, d.name]
    if (d.format === 'json') parts.push('JSON')
    if (d.kms_key_id) parts.push(`KMS ${d.kms_key_id}`)
    if (d.replace) parts.push('replaces')
    return parts.join(' · ')
  }
  const place = [d.org ?? 'Personal', d.org ? d.collection : d.folder].filter(Boolean).join(' / ')
  const kind = d.item === 'secure_note' ? 'secure note' : 'login'
  return `Bitwarden · ${d.connection} › ${place} · ${d.replace ? 'replaces' : 'new'} ${kind} "${d.name}"`
}

/** Whether a file of `bytes` fits `send_max_file_mb` (D20). Unknown size: allowed, the engine checks again. */
export const fileFits = (bytes: number | null, maxMb: number): boolean => bytes === null || bytes <= maxMb * 1024 * 1024

/** The server's Premium refusal for a file Send (§12.3's mapped error). */
export const premiumRefusal = (message: string): boolean => /file Sends need Premium/i.test(message)

/** `1.2 MB` */
export function bytesLabel(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}
