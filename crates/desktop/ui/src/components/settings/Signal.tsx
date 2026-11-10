import { useQuery } from '@tanstack/react-query'
import { Copy } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { toast } from 'sonner'
import { AllowList, useChannelEvents } from '@/components/settings/Channels'
import { Dot, Row, Section, useAction } from '@/components/settings/shared'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SignalStatus } from '@/rpc/bindings/SignalStatus'

function signalState(s: SignalStatus): string {
  const e = s.state
  if (!e) return s.registered ? 'registered' : 'not linked'
  switch (e.type) {
    case 'LinkUrl':
      return 'waiting for the link'
    case 'Linked':
      return `linked as ${e.number}`
    case 'Connected':
      return 'connected'
    case 'Disconnected':
      return `disconnected, retrying in ${e.retry_in_secs}s`
    case 'Unlinked':
      return 'unlinked'
    case 'DaemonOnly':
      return 'daemon only'
  }
}

/** 188: Channels › Signal: the linked account, linking this device, the owner and allowed numbers. */
export function Signal() {
  const api = useApi()
  useChannelEvents(queryKeys.signal)
  const status = useQuery({ queryKey: queryKeys.signal, queryFn: () => api.signal_status() })
  const keys = [queryKeys.signal]
  const link = useAction((_: void) => api.signal_link())
  const unlink = useAction((_: void) => api.signal_unlink(), { keys, onSuccess: () => link.reset() })
  const allow = useAction((n: string) => api.signal_allow(n), { keys })
  const remove = useAction((n: string) => api.signal_remove_allowed(n), { keys })
  const s = status.data
  const linkUrl = link.data ?? (s?.state?.type === 'LinkUrl' ? s.state.url : null)

  return (
    <Section title="Signal">
      {s && (
        <>
          <Row
            label="Account"
            description={
              <span className="flex items-center gap-1.5">
                <Dot on={s.state?.type === 'Connected' || s.state?.type === 'Linked'} />
                {s.number ? `${s.number} · ${signalState(s)}` : signalState(s)}
              </span>
            }
          >
            {s.registered ? (
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button variant="outline" size="sm">
                    Unlink
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>Unlink Signal?</AlertDialogTitle>
                    <AlertDialogDescription>
                      This device is removed from the account; linking again needs a new QR scan.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction onClick={() => unlink.mutate(undefined)}>Unlink</AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            ) : (
              <Button variant="outline" size="sm" disabled={link.isPending} onClick={() => link.mutate(undefined)}>
                Link
              </Button>
            )}
          </Row>
          {linkUrl && !s.registered && (
            <Row
              label="Link this device"
              description={
                <>
                  Open Signal › Linked devices › Link new device, and scan this code:
                  <div className="mt-3 inline-block rounded-lg bg-white p-3">
                    <QRCodeSVG value={linkUrl} size={192} marginSize={0} />
                  </div>
                  <code className="mt-2 block break-all font-mono text-xs text-fg-tertiary">{linkUrl}</code>
                </>
              }
            >
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Copy link"
                onClick={() => navigator.clipboard.writeText(linkUrl).then(() => toast.message('Copied'))}
              >
                <Copy />
              </Button>
            </Row>
          )}
          <Row label="Owner" description="Messages from this number always reach the agent.">
            <span className="font-mono text-sm">{s.owner}</span>
          </Row>
          <AllowList
            label="Allowed numbers"
            items={s.allowed}
            placeholder="+15551234567"
            allow={allow}
            remove={remove}
          />
        </>
      )}
    </Section>
  )
}
