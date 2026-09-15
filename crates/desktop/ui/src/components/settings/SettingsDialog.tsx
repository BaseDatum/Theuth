import { cn } from 'cn'
import {
  Bot,
  KeyRound,
  Keyboard,
  type LucideIcon,
  MessageSquare,
  Settings2,
  ShieldCheck,
  Sparkles,
  Wrench,
} from 'lucide-react'
import { Approvals } from '@/components/settings/Approvals'
import { Channels } from '@/components/settings/Channels'
import { Connections } from '@/components/settings/Connections'
import { General } from '@/components/settings/General'
import { Harnesses } from '@/components/settings/Harnesses'
import { Shortcuts } from '@/components/settings/Shortcuts'
import { Skills } from '@/components/settings/Skills'
import { Tools } from '@/components/settings/Tools'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'

const TABS = [
  ['general', 'General', Settings2],
  ['approvals', 'Approvals', ShieldCheck],
  ['tools', 'Tools', Wrench],
  ['skills', 'Skills', Sparkles],
  ['harnesses', 'Harnesses', Bot],
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

/** ChatGPT-style two-pane settings modal; the open tab is the route param. */
export function SettingsDialog({ tab, session, onTab, onClose }: Props) {
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        onOpenAutoFocus={(e) => e.preventDefault()}
        className="flex h-[640px] max-w-[880px] flex-row gap-0 overflow-hidden bg-elevated p-0 sm:max-w-[880px]"
      >
        <DialogTitle className="sr-only">Settings</DialogTitle>
        <nav className="flex w-[220px] shrink-0 flex-col gap-0.5 border-r border-border-subtle px-3 pt-5 pb-3">
          <h2 className="mb-3 px-2.5 text-lg font-semibold">Settings</h2>
          {TABS.map(([id, label, Icon]) => (
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
              {label}
            </button>
          ))}
        </nav>
        {/* Not ScrollArea: Radix wraps the viewport in `display: table`, which never shrinks, so rows overflowed. */}
        <div className="min-w-0 flex-1 overflow-y-auto">
          <div className="px-8 pt-6 pb-8">
            {tab === 'general' && <General />}
            {tab === 'approvals' && <Approvals session={session} onClose={onClose} />}
            {tab === 'tools' && <Tools session={session} />}
            {tab === 'skills' && <Skills session={session} />}
            {tab === 'harnesses' && <Harnesses />}
            {tab === 'connections' && <Connections />}
            {tab === 'channels' && <Channels />}
            {tab === 'shortcuts' && <Shortcuts />}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
