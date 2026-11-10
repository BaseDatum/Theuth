import { cn } from 'cn'
import { ChevronLeft, ChevronRight, X } from 'lucide-react'
import { useState } from 'react'
import { CATEGORIES, placeOf } from '@/components/settings/registry'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'

/** 188 C0: every tab of every category, in column order; the interim modal lists them flat. */
const TABS = CATEGORIES.flatMap((category) => category.tabs.map((tab) => ({ ...tab, category: category.label })))

type Props = {
  /** A registry tab id (the route has already redirected old ids). */
  tab: string
  /** A control the tab focuses on open (`?focus=`); Memory: `model` (the compactor model picker) or `spend` (the daily spend cap). */
  focus: string | null
  onTab: (tab: string) => void
  onClose: () => void
}

function Pane({ tab, focus }: Pick<Props, 'tab' | 'focus'>) {
  const Tab = placeOf(tab)?.tab.Component
  return Tab ? <Tab focus={focus} /> : null
}

/**
 * 188 C0: the old two-pane modal, now driven by the settings registry so no file but a category module imports a tab
 * component. W-SHELL replaces it with the full settings page (docs/design/188-settings-full-page.md).
 */
export function SettingsDialog({ tab, focus, onTab, onClose }: Props) {
  const mobile = useIsMobile()
  const [pushed, setPushed] = useState(false)
  const label = placeOf(tab)?.tab.label ?? 'Settings'

  if (mobile) {
    return (
      <Sheet open onOpenChange={(open) => !open && onClose()}>
        {/* The X lives in the header row, not the sheet's absolute one: that one lands on the first row's chevron. */}
        <SheetContent side="bottom" showCloseButton={false} className="top-0 h-band gap-0 bg-elevated pt-[var(--safe-top)]">
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
            <SheetClose className="ml-auto flex size-11 items-center justify-center rounded-full text-fg-secondary active:bg-layer-3">
              <X className="size-5" />
              <span className="sr-only">Close</span>
            </SheetClose>
          </div>
          {pushed ? (
            <div className="min-h-0 flex-1 overflow-y-auto px-4 pt-4 pb-8">
              <Pane tab={tab} focus={focus} />
            </div>
          ) : (
            <nav className="min-h-0 flex-1 overflow-y-auto p-2">
              {TABS.map(({ id, label: tabLabel, icon: Icon }) => (
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
        <nav className="flex w-[220px] shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-border-subtle px-3 pt-5 pb-3">
          <h2 className="mb-3 px-2.5 text-lg font-semibold">Settings</h2>
          {TABS.map(({ id, label: tabLabel, icon: Icon, category }) => (
            <button
              key={id}
              type="button"
              title={category}
              onClick={() => onTab(id)}
              className={cn('flex h-9 shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 text-sm', id === tab ? 'bg-layer-3' : 'hover:bg-layer-3/60')}
            >
              <Icon className="size-4 text-fg-secondary" />
              {tabLabel}
            </button>
          ))}
        </nav>
        {/* Not ScrollArea: Radix wraps the viewport in `display: table`, which never shrinks, so rows overflowed. */}
        <div className="min-w-0 flex-1 overflow-y-auto">
          <div className="px-8 pt-6 pb-8">
            <Pane tab={tab} focus={focus} />
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
