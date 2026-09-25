import { BookOpen, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { SkillBody } from '@/components/library/Skills'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { ActionButton } from './MessageAssistant'

/** A skill injection in the transcript: a tool-card-width row (settings' Sparkles icon, "Skill … added") whose
 * open button shows the skill's markdown in a side panel for a quick read. */
export function SkillCard({ name }: { name: string }) {
  const [open, setOpen] = useState(false)
  return (
    <div className="my-2 flex w-full min-w-0 items-center gap-2 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <Sparkles className="size-4 shrink-0 text-fg-secondary" />
      <span className="min-w-0 flex-1 wrap-anywhere">
        Skill <span className="font-medium">{name}</span> added
      </span>
      <ActionButton icon={BookOpen} label="Open skill" onClick={() => setOpen(true)} />
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent className="flex w-full flex-col sm:w-[560px] sm:max-w-[560px]">
          <SheetHeader>
            <SheetTitle className="flex items-center gap-2">
              <Sparkles className="size-4" /> {name}
            </SheetTitle>
          </SheetHeader>
          <SkillBody name={name} />
        </SheetContent>
      </Sheet>
    </div>
  )
}
