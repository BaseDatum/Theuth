// 158 §9.2 (W3): the project combobox — the start form's "Project" row and the meeting page's header chip. Rows are
// the known projects (`useProjectChoices`), `empty` (the default / no project), and a "New project ‹name›" row when
// the typed text matches none; picking that row is what creates the label (§9.1: a project is only a string).

import { Check, ChevronDown, Plus } from 'lucide-react'
import { useState } from 'react'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { newProjectName, useProjectChoices } from '@/lib/meeting-project'

export function ProjectChip({
  value,
  onChange,
  empty,
  disabled,
  variant = 'outline',
  className,
}: {
  value: string | null
  onChange(v: string | null): void
  /** The `null` row's label: "Default" on the start form (the engine resolves it), "No project" on a meeting. */
  empty: string
  disabled?: boolean
  variant?: 'outline' | 'ghost'
  className?: string
}) {
  const projects = useProjectChoices()
  const [open, setOpen] = useState(false)
  const [typed, setTyped] = useState('')
  const fresh = newProjectName(typed, projects)
  const pick = (v: string | null) => {
    if (v !== value) onChange(v)
    setTyped('')
    setOpen(false)
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button type="button" variant={variant} size="sm" role="combobox" aria-expanded={open} aria-label="Project" disabled={disabled} className={className}>
          <ProjectIcon project={value} className="size-3.5" />
          <span className="truncate font-normal">{value ?? empty}</span>
          <ChevronDown className="size-4 opacity-70" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-0">
        <Command>
          <CommandInput placeholder="Find or name a project" value={typed} onValueChange={setTyped} />
          <CommandList>
            <CommandEmpty>Type a name to create a project.</CommandEmpty>
            <CommandGroup>
              <CommandItem value="none" onSelect={() => pick(null)}>
                {empty}
                {value === null && <Check className="ml-auto size-4" />}
              </CommandItem>
              {projects.map((p) => (
                <CommandItem key={p} value={p} onSelect={() => pick(p)}>
                  <ProjectIcon project={p} className="size-3.5" />
                  <span className="truncate">{p}</span>
                  {value === p && <Check className="ml-auto size-4" />}
                </CommandItem>
              ))}
            </CommandGroup>
            {fresh && (
              <>
                <CommandSeparator />
                <CommandGroup forceMount>
                  <CommandItem value={`new:${fresh}`} forceMount onSelect={() => pick(fresh)}>
                    <Plus className="size-3.5" />
                    New project <span className="font-medium">{fresh}</span>
                  </CommandItem>
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}
