import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { COLORS, ICONS, LEGACY_COLORS, colorFor, iconFor, useProjectStyles } from '@/lib/project-style'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { ProjectStyle } from "@/rpc/bindings/method.agent.output"

/**
 * A project's glyph in its colour (51 §3 as-built); a neutral `Folder` in `text-fg-tertiary` while unstyled, so the
 * slot never appears or disappears. `project === null` (Ungrouped) gets the same neutral glyph.
 */
export function ProjectIcon({ project, className }: { project: string | null; className?: string }) {
  const style = useProjectStyles()[project ?? '']
  const Icon = iconFor(style?.icon)
  return <Icon className={cn('shrink-0', colorFor(style?.color), className)} aria-hidden />
}

/**
 * The project header's "Icon & colour…" dialog: a grid of the allow-listed icons, a row of palette swatches and
 * Clear. Every tap applies at once (optimistic on the `projectStyles` cache, then a refetch); 44 px targets for touch.
 */
export function ProjectStyleDialog({ project, open, onOpenChange }: { project: string; open: boolean; onOpenChange(open: boolean): void }) {
  const api = useApi()
  const qc = useQueryClient()
  const current = useProjectStyles()[project]
  const [busy, setBusy] = useState(false)

  const apply = async (icon: string | null, color: string | null) => {
    setBusy(true)
    qc.setQueryData<ProjectStyle[]>(queryKeys.projectStyles, (rows = []) => {
      const rest = rows.filter((r) => r.name !== project)
      return icon || color ? [...rest, { name: project, icon, color }] : rest
    })
    try {
      await api.set_project_style(project, icon, color)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
      void qc.invalidateQueries({ queryKey: queryKeys.projectStyles })
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-fit max-w-[calc(100%-2rem)] gap-3 p-4">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-sm">
            <ProjectIcon project={project} className="size-4" />
            <span className="truncate">{project}</span>
          </DialogTitle>
        </DialogHeader>
        <div role="radiogroup" aria-label="Icon" className="grid grid-cols-6 gap-0.5">
          {Object.entries(ICONS).map(([name, Icon]) => (
            <button
              key={name}
              type="button"
              role="radio"
              aria-checked={current?.icon === name}
              aria-label={name}
              disabled={busy}
              className={cn(
                'flex size-11 items-center justify-center rounded-md hover:bg-accent',
                current?.icon === name && 'bg-accent ring-1 ring-ring',
              )}
              onClick={() => void apply(name, current?.color ?? null)}
            >
              <Icon className={cn('size-5', colorFor(current?.color))} />
            </button>
          ))}
        </div>
        <div role="radiogroup" aria-label="Colour" className="flex flex-wrap gap-0.5">
          {Object.keys(COLORS).map((key) => {
            const selected = current?.color != null && (LEGACY_COLORS[current.color] ?? current.color) === key
            const label = key[0]!.toUpperCase() + key.slice(1)
            return (
              <button
                key={key}
                type="button"
                role="radio"
                aria-checked={selected}
                aria-label={label}
                title={label}
                disabled={busy}
                className="flex size-11 items-center justify-center rounded-md hover:bg-accent"
                onClick={() => void apply(current?.icon ?? null, key)}
              >
                <span className={cn('size-5 rounded-full', `bg-project-${key}`, selected && 'ring-2 ring-ring ring-offset-2 ring-offset-background')} />
              </button>
            )
          })}
        </div>
        <Button variant="ghost" size="sm" className="self-start" disabled={busy || !current} onClick={() => void apply(null, null)}>
          Clear
        </Button>
      </DialogContent>
    </Dialog>
  )
}
