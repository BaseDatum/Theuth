import { AppWindow } from 'lucide-react'
import { notify as toast } from '@/lib/notify'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { type PopoutTarget, canPopout, openPopout } from '@/lib/popout'

/** A transcript card's Open in new window (`target` alone in its own window); nothing where there are no windows (iOS). */
export function PopoutButton({ target }: { target: PopoutTarget }) {
  if (!canPopout()) return null
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-fg-secondary"
          aria-label="Open in new window"
          onClick={() => void openPopout(target).catch((e: unknown) => toast.error(String(e)))}
        >
          <AppWindow className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>Open in new window</TooltipContent>
    </Tooltip>
  )
}
