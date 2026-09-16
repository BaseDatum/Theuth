import { cn } from 'cn'
import {
  Bot,
  ChevronLeft,
  ChevronRight,
  KeyRound,
  Keyboard,
  Link2,
  type LucideIcon,
  MessageSquare,
  Monitor,
  Server,
  Settings2,
  ShieldCheck,
  Sparkles,
  Wrench,
} from 'lucide-react'
import { useState } from 'react'
import { Harnesses } from '@/components/link/Harnesses'
import { Agents } from '@/components/settings/Agents'
import { Approvals } from '@/components/settings/Approvals'
import { Channels } from '@/components/settings/Channels'
import { Computer } from '@/components/settings/Computer'
import { Connections } from '@/components/settings/Connections'
import { General } from '@/components/settings/General'
import { Link } from '@/components/settings/Link'
import { Shortcuts } from '@/components/settings/Shortcuts'
import { Skills } from '@/components/settings/Skills'
import { Tools } from '@/components/settings/Tools'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'

const TABS = [
  ['general', 'General', Settings2],
  ['approvals', 'Approvals', ShieldCheck],
  ['tools', 'Tools', Wrench],
  ['computer', 'Computer', Monitor],
  ['skills', 'Skills', Sparkles],
  ['agents', 'Agents', Bot],
  ['harnesses', 'Harnesses', Server],
  ['link', 'Link', Link2],
  ['connections', 'Connections', KeyRound],
  ['channels', 'Channels', MessageSquare],
  ['shortcuts', 'Shortcuts', Keyboard],
] as const satisfies readonly (readonly [string, string, LucideIcon])[]

export type Tab = (typeof TABS)[number][0]

export function isTab(s: string): s is Tab {
  return TABS.some(([id]) => id === s)
}

type Props = {
  tab: Tab
  /** The chat the dialog was opened from; `null` from the empty state or the tray. */
  session: string | null
  onTab: (tab: Tab) => void
  onClose: () => void
}

function Pane({ tab, session, onClose }: Omit<Props, 'onTab'>) {
  return (
    <>
      {tab === 'general' && <General />}
      {tab === 'approvals' && <Approvals session={session} onClose={onClose} />}
      {tab === 'tools' && <Tools session={session} />}
      {tab === 'computer' && <Computer />}
      {tab === 'skills' && <Skills session={session} />}
      {tab === 'agents' && <Agents />}
      {tab === 'harnesses' && <Harnesses />}
      {tab === 'link' && <Link />}
      {tab === 'connections' && <Connections />}
      {tab === 'channels' && <Channels />}
      {tab === 'shortcuts' && <Shortcuts />}
    </>
  )
}

/**
 * ChatGPT-style two-pane settings modal; the open tab is the route param.
 * Under the mobile breakpoint (31 §8.1) it is a full-screen sheet: the tab
 * list first, a tap pushes the tab with a back chevron.
 */
export function SettingsDialog({ tab, session, onTab, onClose }: Props) {
  const mobile = useIsMobile()
  const [pushed, setPushed] = useState(false)
  const label = TABS.find(([id]) => id === tab)?.[1] ?? 'Settings'

  if (mobile) {
    return (
      <Sheet open onOpenChange={(open) => !open && onClose()}>
        <SheetContent side="bottom" showCloseButton={!pushed} className="h-dvh gap-0 bg-elevated">
          <SheetTitle className="sr-only">Settings</SheetTitle>
          <div className="flex h-12 shrink-0 items-center gap-1 border-b border-border-subtle px-2">
            {pushed ? (
              <button
                type="button"
                onClick={() => setPushed(false)}
                className="flex h-11 items-center gap-0.5 pr-2 text-sm text-fg-secondary"
              >
                <ChevronLeft className="size-5" />
                Settings
              </button>
            ) : (
              <h2 className="px-2 text-lg font-semibold">Settings</h2>
            )}
            {pushed && <span className="font-semibold">{label}</span>}
          </div>
          {pushed ? (
            <div className="min-h-0 flex-1 overflow-y-auto px-4 pt-4 pb-8">
              <Pane tab={tab} session={session} onClose={onClose} />
            </div>
          ) : (
            <nav className="min-h-0 flex-1 overflow-y-auto p-2">
              {TABS.map(([id, tabLabel, Icon]) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => {
                    onTab(id)
                    setPushed(true)
                  }}
                  className="flex h-12 w-full items-center gap-3 rounded-[10px] px-3 text-base active:bg-layer-3"
                >
                  <Icon className="size-5 text-fg-secondary" />
                  <span className="flex-1 text-left">{tabLabel}</span>
                  <ChevronRight className="size-4 text-fg-tertiary" />
                </button>
              ))}
            </nav>
          )}
        </SheetContent>
      </Sheet>
    )
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        onOpenAutoFocus={(e) => e.preventDefault()}
        className="flex h-[640px] max-w-[880px] flex-row gap-0 overflow-hidden bg-elevated p-0 sm:max-w-[880px]"
      >
        <DialogTitle className="sr-only">Settings</DialogTitle>
        <nav className="flex w-[220px] shrink-0 flex-col gap-0.5 border-r border-border-subtle px-3 pt-5 pb-3">
          <h2 className="mb-3 px-2.5 text-lg font-semibold">Settings</h2>
          {TABS.map(([id, tabLabel, Icon]) => (
            <button
              key={id}
              type="button"
              onClick={() => onTab(id)}
              className={cn(
                'flex h-9 items-center gap-2.5 rounded-[10px] px-2.5 text-sm',
                id === tab ? 'bg-layer-3' : 'hover:bg-layer-3/60',
              )}
            >
              <Icon className="size-4 text-fg-secondary" />
              {tabLabel}
            </button>
          ))}
        </nav>
        {/* Not ScrollArea: Radix wraps the viewport in `display: table`, which never shrinks, so rows overflowed. */}
        <div className="min-w-0 flex-1 overflow-y-auto">
          <div className="px-8 pt-6 pb-8">
            <Pane tab={tab} session={session} onClose={onClose} />
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
