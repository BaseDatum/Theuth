import { Bell, PanelLeft } from 'lucide-react'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import { AttentionList, Section } from './Hud'
import { useAttention } from './queries'

/** `/attention` — the inbox on its own (70 HUD, "both is fine"): the same cards as the HUD's first section. */
export function AttentionView() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const attention = useAttention()
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
          Attention {attention.data && <span className="font-normal text-fg-tertiary tabular-nums">{attention.data.length}</span>}
        </h2>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[880px] p-4">
          <Section icon={<Bell className="size-3.5" />} title="Needs you" pending={attention.isPending} error={attention.error}>
            <AttentionList items={attention.data} />
          </Section>
        </div>
      </div>
    </article>
  )
}
