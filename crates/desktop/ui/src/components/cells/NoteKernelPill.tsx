// Owner: U2
import { MoreHorizontal, Play } from 'lucide-react'
import { type KeyboardEvent, useEffect } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isApple } from '@/lib/chord'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/api'
import type { CellScope } from '@/rpc/bindings/CellScope'
import type { DocId } from '@/rpc/bindings/DocId'
import type { KernelState } from '@/rpc/bindings/KernelState'
import { liveKernels } from './KernelStrip'
import { mountedCells } from './NoteCell'
import { digest } from './grammar'
import type { MountedCell } from './note-edit'
import { errorText, mintId, press, refresh, register, useScope } from './store'

export type NoteKernelPillProps = {
  doc: DocId
}

/** `python · .venv 3.12 · idle`: the kernel's family, the engine's label and the phase in the strip's words. */
export function pillText(k: KernelState): string {
  const family = k.id.spec.type === 'shell' ? k.id.spec.kind : k.id.spec.type
  return [family, k.label, k.phase === 'ready' ? 'idle' : k.phase].join(' · ')
}

/** Note Run all (§7.2, §11.5): one engine pass over the note's cells top to bottom; the engine applies the boundaries. */
export async function runAllNote(api: Pick<Api, 'cell_run_all'>, scope: CellScope, cells: readonly MountedCell[]) {
  const pairs = await Promise.all(cells.map(async (c): Promise<[MountedCell['ref'], string]> => [c.ref, await digest(c.text)]))
  return press(mintId('p_'), (id) => api.cell_run_all(scope, pairs, id))
}

/**
 * 171 §11.5: `python · .venv 3.12 · idle · Run all` beside the note's breadcrumb, one pill per live kernel; Restart
 * and Close in its menu; with no live kernel and cells on the page, one quiet `cells · Run all`. Mod+Shift+Enter
 * while the pill row is focused runs all (§11.7).
 */
export function NoteKernelPill({ doc }: NoteKernelPillProps) {
  const api = useApi()
  const scope: CellScope = { type: 'doc', id: doc }
  useEffect(() => register(api, { type: 'doc', id: doc }, null), [api, doc])
  const slice = useScope(scope)
  const kernels = liveKernels(slice.kernels)
  const cells = Object.keys(slice.states).length
  if (kernels.length === 0 && cells === 0) return null
  const act = (f: () => Promise<unknown>) =>
    f().then(
      () => refresh(scope),
      (e: unknown) => toast.error(errorText(e)),
    )
  const runAll = () => act(() => runAllNote(api, scope, mountedCells(doc)))
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && e.shiftKey && (isApple() ? e.metaKey : e.ctrlKey)) {
      e.preventDefault()
      void runAll()
    }
  }
  const runAllButton = (
    <Button variant="ghost" size="sm" className="h-5 gap-1 rounded-full px-1.5 text-[11px] text-fg-accent pointer-coarse:h-12" onClick={() => void runAll()} title={`Run all · ${isApple() ? '⌘⇧↵' : 'Ctrl+Shift+Enter'}`}>
      <Play className="size-2.5 fill-current" aria-hidden />
      Run all
    </Button>
  )
  return (
    <div role="group" aria-label="Note kernels" className="flex min-w-0 shrink items-center gap-1" onKeyDown={onKey}>
      {kernels.length === 0 ? (
        <span className="flex items-center gap-1 rounded-full border border-border-subtle py-px pr-0.5 pl-2 text-[11px] text-fg-tertiary">cells{runAllButton}</span>
      ) : (
        kernels.map((k) => {
          const warning = k.env_warning
          return (
            <span key={JSON.stringify(k.id.spec)} className="flex min-w-0 items-center gap-1 rounded-full border border-border-subtle py-px pr-0.5 pl-2 text-[11px] text-fg-secondary">
              <span className={cn('size-1.5 shrink-0 rounded-full', k.phase === 'busy' ? 'bg-fg-accent' : 'bg-success')} aria-hidden />
              <span className="truncate">{pillText(k)}</span>
              {warning && (
                <Button variant="ghost" size="sm" className="h-5 rounded-full px-1.5 text-[11px] text-warning pointer-coarse:h-12" title={warning} onClick={() => void act(() => api.cell_env_refresh())}>
                  your shell's environment could not be read — Refresh
                </Button>
              )}
              {runAllButton}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon" className="size-5 rounded-full pointer-coarse:size-12" aria-label={`${pillText(k)} menu`}>
                    <MoreHorizontal className="size-3" aria-hidden />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => void act(() => api.cell_kernel_restart(k.id))}>Restart</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => void act(() => api.cell_kernel_close(k.id))}>Close</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </span>
          )
        })
      )}
    </div>
  )
}
