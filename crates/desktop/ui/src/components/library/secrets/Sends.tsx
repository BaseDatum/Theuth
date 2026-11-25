import { useQuery } from '@tanstack/react-query'
import { open as pickFile } from '@tauri-apps/plugin-dialog'
import { useState } from 'react'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { notify } from '@/lib/notify'
import { isTauri } from '@/lib/tauri'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { bytesLabel } from './labels'
import { SendDialog, type SendSource, copySendLink } from './SendDialog'
import type { ConnInfo } from '@/rpc/bindings/method.secrets.output'

const when = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })

/**
 * §12.5 Sends tab of a Bitwarden connection: the Sends theuth created (all with `manage_all_sends`), never a link. Copy
 * link writes it to the clipboard through `copy_send_link`; Revoke deletes the Send and its link secret. Send a file…
 * opens the native picker, which yields an absolute path, then the Send dialog.
 */
export function Sends({ info }: { info: ConnInfo }) {
  const api = useApi()
  const can = useQuerySupport(['list_sends', 'revoke_send', 'copy_send_link'])
  const list = useQuery({ enabled: can, queryKey: queryKeys.secretSends(info.conn), queryFn: () => api.list_sends(info.conn) })
  const [sending, setSending] = useState<SendSource | null>(null)
  const revoke = useAction((id: string) => api.revoke_send(id), { keys: [queryKeys.secretSends(info.conn)] })
  if (!can) return null
  return (
    <div className="mb-3 ml-4 flex flex-col gap-1 border-l border-border-subtle pl-4 text-[13px]">
      {sending && <SendDialog open initial={sending} onOpenChange={(o) => !o && setSending(null)} />}
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium text-fg-tertiary">Sends</span>
        {isTauri() && (
          <Button
            variant="outline"
            size="sm"
            onClick={() =>
              void pickFile({ multiple: false, directory: false }).then(
                (path) => typeof path === 'string' && setSending({ kind: 'file', file: path, label: path, bytes: null }),
                (e) => notify.error(errorText(e)),
              )
            }
          >
            Send a file…
          </Button>
        )}
      </div>
      {list.data?.length === 0 && <Empty>No Send yet.</Empty>}
      {list.data?.map((s) => (
        <div key={s.id} className="flex items-center justify-between gap-3 border-b border-border-subtle py-1.5 last:border-0">
          <div className="min-w-0">
            <div className="truncate">
              {/* `file_bytes` is null for a text sent as a file (and absent from an older engine): no size then. */}
              {s.kind === 'file' && s.file_name ? `${s.file_name}${s.file_bytes != null ? ` · ${bytesLabel(s.file_bytes)}` : ''}` : s.name} <span className="text-fg-tertiary">· {s.status.replace(/_/g, ' ')}</span>
            </div>
            <div className="truncate text-xs text-fg-tertiary">
              {s.source_ref} · opened {s.access_count}
              {s.max_access !== null && ` of ${s.max_access}`} · {s.expiration_ms !== null ? `expires ${when(s.expiration_ms)}` : `deleted ${when(s.deletion_ms)}`}
              {s.has_password && ' · password'}
            </div>
          </div>
          {s.status === 'active' && (
            <div className="flex shrink-0 gap-1">
              <Button variant="outline" size="sm" onClick={() => void copySendLink(api, s.id)}>
                Copy link
              </Button>
              <Button variant="ghost" size="sm" disabled={revoke.isPending} onClick={() => confirm(`Revoke ${s.link_secret ?? s.name}?`) && revoke.mutate(s.id)}>
                Revoke
              </Button>
            </div>
          )}
        </div>
      ))}
    </div>
  )
}
