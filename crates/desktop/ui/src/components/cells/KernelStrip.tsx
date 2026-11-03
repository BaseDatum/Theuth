// Owner: U1
import { openPath } from '@tauri-apps/plugin-opener'
import { ChevronDown, MoreHorizontal } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isDesktop } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/api'
import type { KernelState } from '@/rpc/bindings/KernelState'
import { useConnection } from '@/store/connection'
import { errorText, refresh, register, useScope } from './store'

export type KernelStripProps = {
  session: string
}

/** Kernels the strip shows: a process exists (§5.3 `starting`/`ready`/`busy`). */
export const liveKernels = (ks: KernelState[]) => ks.filter((k) => k.phase === 'starting' || k.phase === 'ready' || k.phase === 'busy')

const phaseText = (k: KernelState) => (k.phase === 'ready' ? 'idle' : k.phase)

function KernelRow({ kernel, api, onError }: { kernel: KernelState; api: Api; onError(text: string | null): void }) {
  const act = (f: () => Promise<unknown>) =>
    f().then(
      () => refresh(kernel.id.scope),
      (e: unknown) => toast.error(errorText(e)),
    )
  const dataDir = useConnection((s) => s.info?.data_dir)
  return (
    <div className="flex min-w-0 items-center gap-2 text-xs">
      <span
        aria-hidden
        className={cn('size-2 shrink-0 rounded-full', kernel.phase === 'busy' || kernel.phase === 'starting' ? 'bg-fg-accent motion-safe:animate-pulse' : 'bg-success')}
      />
      <span className="shrink-0 text-fg-secondary">
        {kernel.label} · {phaseText(kernel)}
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

/** 171 §11.4: the session's live kernels on top of the composer; renders nothing without one. */
export function KernelStrip({ session }: KernelStripProps) {
  const api = useApi()
  const scope = { type: 'session', id: session } as const
  useEffect(() => register(api, { type: 'session', id: session }, null), [api, session])
  const kernels = liveKernels(useScope(scope).kernels)
  const [open, setOpen] = useState(false)
  const [envError, setEnvError] = useState<string | null>(null)
  if (kernels.length === 0) return null
  const [first] = kernels
  return (
    <div className="mx-auto w-full shrink-0 px-4 md:max-w-[768px]" role="region" aria-label="Kernels">
      <div className="flex flex-col gap-1 rounded-t-xl border border-b-0 border-border bg-layer-1 px-3 py-1.5">
        {envError && (
          <span className="self-start rounded-md border border-warning-border bg-warning-bg px-1.5 text-[11px] text-warning" title={envError}>
            your shell's environment could not be read — Refresh
          </span>
        )}
        {kernels.length === 1 && first ? (
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
