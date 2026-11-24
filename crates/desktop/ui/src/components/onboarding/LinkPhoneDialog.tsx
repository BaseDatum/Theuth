import './wizard.css'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { CircleCheck, Copy, LoaderCircle, MessagesSquare, Monitor, RefreshCw } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { create } from 'zustand'
import { settings } from '@/components/chat/slash'
import { Countdown } from '@/components/settings/Link'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { on } from '@/store/sessions'
import { isNewPhone } from './steps/content'

/** Open state of the dialog: the checklist's phone item and the end of the tour open it with `openLinkPhone()`. */
export const useLinkPhone = create<{ open: boolean }>()(() => ({ open: false }))
export const openLinkPhone = (): void => useLinkPhone.setState({ open: true })

/** 215 step 5: pair a phone with this mesh by QR (`device_pair_start`), or go to the other ways in. */
export function LinkPhoneDialog() {
  const open = useLinkPhone((s) => s.open)
  const navigate = useNavigate()
  const can = useQuerySupport(['device_pair_start'])
  const close = () => useLinkPhone.setState({ open: false })
  const go = (tab: 'machines' | 'slack') => {
    close()
    void settings(navigate, tab)
  }
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="gap-6 rounded-2xl border-border-subtle bg-layer-1 p-8 sm:max-w-[520px]">
        {open && (can ? <Pairing onDone={close} /> : <Unsupported />)}
        <DialogFooter className="flex-row flex-wrap gap-2 border-t border-border-subtle pt-5 sm:justify-start">
          <span className="w-full text-[13px] text-fg-tertiary">Other ways in</span>
          <Button variant="outline" size="sm" onClick={() => go('machines')}>
            <Monitor /> Another computer
          </Button>
          <Button variant="outline" size="sm" onClick={() => go('slack')}>
            <MessagesSquare /> Slack, Telegram or Signal
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Unsupported() {
  return (
    <DialogHeader>
      <DialogTitle>Link your phone</DialogTitle>
      <DialogDescription>This engine cannot pair devices from the app. Pair from Settings › Network › Devices on the engine's computer.</DialogDescription>
    </DialogHeader>
  )
}

function Pairing({ onDone }: { onDone(): void }) {
  const api = useApi()
  const qc = useQueryClient()
  const canList = useQuerySupport(['devices_list'])
  const start = useMutation({ mutationFn: () => api.device_pair_start(null) })
  const [paired, setPaired] = useState(false)
  const [now, setNow] = useState(Date.now)
  // StrictMode runs effects twice: one dialog open makes one code. `opened` dates this pairing: only a phone paired
  // after it counts (the events carry no pairing id).
  const opened = useRef<number | null>(null)
  const { mutate } = start
  useEffect(() => {
    if (opened.current !== null) return
    opened.current = Date.now()
    mutate()
  }, [mutate])
  useEffect(() => {
    const check = (d: { paired_ms: number; platform: string }) => {
      if (!isNewPhone(d, opened.current ?? Number.POSITIVE_INFINITY)) return
      setPaired(true)
      void qc.invalidateQueries({ queryKey: ['mesh', 'devices'] })
    }
    const offs = [
      on('link.events@1', 'LinkPaired', (e) => check({ paired_ms: e.device.paired_at_ms, platform: e.device.platform })),
      // `DevicePaired` names only the id: the registry row says when and what it is.
      on('link.events@1', 'DevicePaired', (e) => {
        if (!canList) return
        void api.devices_list().then((list) => {
          const d = list.find((x) => x.id === e.id)
          if (d && !d.revoked) check(d)
        }, () => undefined)
      }),
    ]
    return () => {
      for (const off of offs) off()
    }
  }, [api, qc, canList])
  const p = start.data
  // The QR is spent at expiry; one tick a second is enough to swap in "New code".
  useEffect(() => {
    if (!p || paired) return
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [p, paired])
  const expired = !!p && now >= p.expires_ms
  if (paired)
    return (
      <div role="status" className="onb-pop flex flex-col items-center gap-4 py-6 text-center">
        <CircleCheck className="size-10 text-success" aria-hidden />
        <DialogTitle className="text-xl">Your phone is linked</DialogTitle>
        <DialogDescription>Your chats, Inbox and Library are on it now, wherever you are.</DialogDescription>
        <Button onClick={onDone} autoFocus>
          Done
        </Button>
      </div>
    )
  return (
    <>
      <DialogHeader className="gap-2">
        <DialogTitle className="text-xl">Link your phone</DialogTitle>
        <DialogDescription className="text-[15px] leading-relaxed">Reach your agents from anywhere. Scan the code with your phone's camera.</DialogDescription>
      </DialogHeader>
      <div className="flex flex-col items-center gap-4">
        <div className={`flex size-[236px] items-center justify-center rounded-xl p-2 ${p ? 'bg-white' : 'bg-layer-2'} ${expired ? 'opacity-30' : ''}`}>
          {p ? <QRCodeSVG value={p.string} size={220} marginSize={2} aria-label="Pairing code" role="img" /> : start.isError ? null : <LoaderCircle className="size-6 animate-spin text-fg-tertiary motion-reduce:animate-none" aria-label="Making a code" />}
        </div>
        {start.isError && (
          <div role="alert" className="flex flex-col items-center gap-2 text-center text-[13px] text-danger">
            Could not make a pairing code: {errorText(start.error)}
            <Button variant="outline" size="sm" onClick={() => mutate()} disabled={start.isPending}>
              <RefreshCw /> Try again
            </Button>
          </div>
        )}
        {p && (
          <div className="flex items-center gap-3 text-[13px] text-fg-secondary tabular-nums">
            {expired ? 'This code expired.' : <Countdown until={p.expires_ms} />}
            {expired || start.isError ? (
              <Button variant="outline" size="sm" onClick={() => mutate()} disabled={start.isPending}>
                <RefreshCw /> New code
              </Button>
            ) : (
              <Button
                variant="outline"
                size="sm"
                onClick={() => navigator.clipboard.writeText(p.string).then(() => toast.message('Key copied'), () => toast.error('Copy failed'))}
              >
                <Copy /> Copy key
              </Button>
            )}
          </div>
        )}
        <p className="max-w-sm text-center text-[13px] text-fg-tertiary">The iPhone app currently pairs by key: copy it here, then paste it into the app's Pair screen. A code pairs one device and lasts five minutes.</p>
      </div>
    </>
  )
}
