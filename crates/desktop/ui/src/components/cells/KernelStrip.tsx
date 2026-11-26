// Owner: U1
import { openPath } from '@tauri-apps/plugin-opener'
import { ChevronDown, MoreHorizontal } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isDesktop } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import { backgroundColors } from '@/lib/background-colors'
import { CategoryMark } from '@/components/chat/CategoryMark'
import type { Api } from '@/rpc/bindings/client'
import type { KernelState } from "@/rpc/bindings/method.cells.output"
import { useConnection } from '@/store/connection'
import { errorText, refresh, register, useScope } from './store'

export type KernelStripProps = {
  session: string
  /** The body of the bar's Kernels card (`BackgroundArea`). */
  inBar?: boolean
}

/** Kernels the bar shows: a process exists (§5.3 `starting`/`ready`/`busy`). */
export const liveKernels = (ks: KernelState[]) => ks.filter((k) => k.phase === 'starting' || k.phase === 'ready' || k.phase === 'busy')
export const phaseText = (k: KernelState) => (k.phase === 'ready' ? 'idle' : k.phase)

/**
 * #6: the kernel's label with its instance — `zsh 5.9 #2` for the second instance of a shell (started for a run while
 * the first was busy); the first instance is just its label.
 */
export const kernelName = (k: KernelState) => (k.id.instance > 0 ? `${k.label} #${k.id.instance + 1}` : k.label)

/** `inBar`: a row of the bar's panel, marked like every other row there (`CategoryMark`) instead of the status dot. */
function KernelRow({ kernel, api, onError, inBar = false }: { kernel: KernelState; api: Api; onError(text: string | null): void; inBar?: boolean }) {
  const act = (f: () => Promise<unknown>) =>
    f().then(
      () => refresh(kernel.id.scope),
      (e: unknown) => toast.error(errorText(e)),
    )
  const dataDir = useConnection((s) => s.info?.data_dir)
  return (
    <div className="flex min-w-0 items-center gap-2 text-xs">
      {inBar ? (
        <CategoryMark color={cn(backgroundColors.kernels.bg, kernel.phase !== 'ready' && 'motion-safe:animate-pulse')} />
      ) : (
        <span
          aria-hidden
          className={cn('size-2 shrink-0 rounded-full', kernel.phase === 'busy' || kernel.phase === 'starting' ? 'bg-fg-accent motion-safe:animate-pulse' : 'bg-success')}
        />
      )}
      <span className="shrink-0 text-fg-secondary">
        {kernelName(kernel)} · {phaseText(kernel)}
      </span>
      {kernel.cwd && (
        <span className="min-w-0 truncate font-mono text-fg-tertiary" title={kernel.cwd}>
          {kernel.cwd}
        </span>
      )}
      <span className="ml-auto flex shrink-0 items-center">
        <Button variant="ghost" size="sm" className="h-6 px-2 text-xs pointer-coarse:h-12" onClick={() => void act(() => api.cell_kernel_restart(kernel.id))}>
          Restart
        </Button>
        <Button variant="ghost" size="sm" className="h-6 px-2 text-xs pointer-coarse:h-12" onClick={() => void act(() => api.cell_kernel_close(kernel.id))}>
          Close
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="sm" className="size-6 p-0 pointer-coarse:size-12" aria-label="Kernel menu">
              <MoreHorizontal className="size-3.5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              onSelect={() =>
                void api.cell_env_refresh().then(
                  () => {
                    onError(null)
                    toast('Environment refreshed', { action: { label: 'Restart', onClick: () => void act(() => api.cell_kernel_restart(kernel.id)) } })
                  },
                  (e: unknown) => onError(errorText(e)),
                )
              }
            >
              Refresh environment
            </DropdownMenuItem>
            {isDesktop() && dataDir && (
              <DropdownMenuItem onSelect={() => void openPath(`${dataDir}/cells/kernels`).catch((e: unknown) => toast.error(errorText(e)))}>Open log folder</DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </span>
    </div>
  )
}

/**
 * The session's live kernels, read now and re-read on the engine's hints: the bar's dashes, tile and Kernels card each
 * call it (one read per commit, `schedule` coalesces).
 */
export function useSessionKernels(session: string): KernelState[] {
  const api = useApi()
  useEffect(() => register(api, { type: 'session', id: session }, null), [api, session])
  const all = useScope({ type: 'session', id: session }).kernels
  return useMemo(() => liveKernels(all), [all])
}

/**
 * 171 §11.4: the session's live kernels and their actions; renders nothing without one. `inBar`: the body of the bar's
 * Kernels card, which is the frame and the disclosure, so no border of its own and every row shown.
 */
export function KernelStrip({ session, inBar = false }: KernelStripProps) {
  const api = useApi()
  const kernels = useSessionKernels(session)
  const [open, setOpen] = useDisclosure(disclosureKey(session, 'kernels'))
  const [envError, setEnvError] = useState<string | null>(null)
  if (kernels.length === 0) return null
  const [first] = kernels
  // §5.4: the engine's `env_warning`, else a failed Refresh environment.
  const warning = kernels.map((k) => k.env_warning).find(Boolean) ?? envError
  return (
    <div className={cn('w-full shrink-0', !inBar && 'mx-auto px-4 md:max-w-[768px]')} role="region" aria-label="Kernels">
      <div className={cn('flex flex-col gap-1', !inBar && 'rounded-t-xl border border-b-0 border-border bg-layer-1 px-3 py-1.5')}>
        {warning && (
          <span className="self-start rounded-md border border-warning-border bg-warning-bg px-1.5 text-[11px] text-warning" title={warning}>
            your shell's environment could not be read — Refresh
          </span>
        )}
        {inBar ? (
          kernels.map((k) => <KernelRow key={JSON.stringify(k.id)} kernel={k} api={api} onError={setEnvError} inBar />)
        ) : kernels.length === 1 && first ? (
          <KernelRow kernel={first} api={api} onError={setEnvError} />
        ) : (
          <>
            <button type="button" className="flex items-center gap-1 self-start text-xs text-fg-secondary pointer-coarse:min-h-12" aria-expanded={open} onClick={() => setOpen(!open)}>
              {kernels.length} kernels
              <ChevronDown className={cn('size-3 transition-transform', open && 'rotate-180')} aria-hidden />
            </button>
            {open && kernels.map((k) => <KernelRow key={JSON.stringify(k.id)} kernel={k} api={api} onError={setEnvError} />)}
          </>
        )}
      </div>
    </div>
  )
}
