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
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellScope } from '@/rpc/bindings/CellScope'
import type { DocId } from '@/rpc/bindings/DocId'
import type { KernelState } from '@/rpc/bindings/KernelState'
import type { PassEnd } from '@/rpc/bindings/PassEnd'
import { liveKernels } from './KernelStrip'
import { mountedCells } from './NoteCell'
import { digest } from './grammar'
import type { MountedCell } from './note-edit'
import { errorText, labelFor, lastPass, mintId, press, refKey, refresh, register, useScope } from './store'

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
 * 171F B11: `Run all stopped before "Deploy" (2/5): <why>` when the last pass stopped early, `null` when it went
 * through or there is none. `cells`: the note's cells top to bottom (the position); the title is the cell's.
 */
export function passLine(pass: PassEnd | null, cells: CellRef[]): string | null {
  const stop = pass?.stopped
  if (!pass || !stop) return null
  const i = cells.findIndex((c) => refKey(c) === refKey(stop.cell))
  const at = i >= 0 ? i + 1 : pass.ran + (stop.before ? 1 : 0)
  return `Run all stopped ${stop.before ? 'before' : 'at'} "${labelFor(stop.cell)}" (${at}/${pass.total}): ${stop.why}`
}

/**
 * 171F4 N5: whether a keydown is Run all for note `doc` from inside its editor — the chord with the focus anywhere in
 * the BlockNote editor (`.bn-editor`: body text or a cell's chrome) that holds `doc`'s cells. The note editor binds
 * no other action to the chord (BlockNote, the window shortcuts, the cell keys: Shift+Enter / Mod+Enter only).
 */
export function runAllChord(e: Pick<globalThis.KeyboardEvent, 'key' | 'shiftKey' | 'metaKey' | 'ctrlKey' | 'target'>, doc: string): boolean {
  if (e.key !== 'Enter' || !e.shiftKey || !(isApple() ? e.metaKey : e.ctrlKey)) return false
  const editor = (e.target as Element | null)?.closest?.('.bn-editor')
  return !!editor?.querySelector(`[data-note-cell][data-doc="${doc}"]`)
}

/**
 * 171 §11.5: `python · .venv 3.12 · idle · Run all` beside the note's breadcrumb, one pill per live kernel; Restart
 * and Close in its menu; with no live kernel and cells on the page, one quiet `cells · Run all`. Mod+Shift+Enter
 * while the pill row is focused, or anywhere in the note's editor (171F4 N5), runs all (§11.7).
 */
export function NoteKernelPill({ doc }: NoteKernelPillProps) {
  const api = useApi()
  const scope: CellScope = { type: 'doc', id: doc }
  useEffect(() => register(api, { type: 'doc', id: doc }, null), [api, doc])
  const act = (f: () => Promise<unknown>) =>
    f().then(
      () => refresh(scope),
      (e: unknown) => toast.error(errorText(e)),
    )
  const runAll = () => act(() => runAllNote(api, scope, mountedCells(doc)))
  // 171F4 N5: ⌘⇧↵ anywhere in the note's editor (body or a cell) is this Run all, as on the pill row.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (!runAllChord(e, doc)) return
      e.preventDefault()
      e.stopPropagation()
      void runAll()
    }
    document.addEventListener('keydown', onKey, true)
    return () => document.removeEventListener('keydown', onKey, true)
  }, [api, doc])
  const slice = useScope(scope)
  const kernels = liveKernels(slice.kernels)
  const cells = Object.keys(slice.states).length
  if (kernels.length === 0 && cells === 0) return null
  const pass = lastPass(slice)
  // `mountedCells` reads the DOM: none in a server render.
  const line = passLine(pass, typeof document === 'undefined' ? [] : mountedCells(doc).map((c) => c.ref))
  const stopKey = pass?.stopped ? refKey(pass.stopped.cell) : ''
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
      {line && (
        <button
          type="button"
          className="min-w-0 truncate rounded px-1 text-left text-[11px] text-danger hover:underline pointer-coarse:min-h-12"
          title={line}
          onClick={() => document.querySelector(`[data-ref-key="${CSS.escape(stopKey)}"]`)?.scrollIntoView({ block: 'center', behavior: 'smooth' })}
        >
          {line}
        </button>
      )}
    </div>
  )
}
