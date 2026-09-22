import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { Streamdown } from 'streamdown'
import { Empty, Section, errorText, toastSync, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SkillMeta } from '@/rpc/bindings/SkillMeta'
import { useSessionField } from '@/store/sessions'
import { draftSkill } from '@/components/chat/slash'

export function Skills({ session }: { session: string | null }) {
  const api = useApi()
  const navigate = useNavigate()
  const skills = useQuery({
    queryKey: queryKeys.skills,
    queryFn: () => api.list_skills(),
  })
  const loaded = useSessionField(session ?? '', 'skills')
  const [shown, setShown] = useState<SkillMeta | null>(null)
  const [url, setUrl] = useState('')

  const addRepo = useAction((u: string) => api.add_skill_repo(u, null), {
    keys: [queryKeys.skills],
    onSuccess: (added) => {
      setUrl('')
      toast.success(`Added ${added.length} skills`)
    },
  })
  const sync = useAction((_: void) => api.sync_skills(), {
    keys: [queryKeys.skills],
    onSuccess: toastSync,
  })
  // Park the skill on the chat's composer (a new chat when none is open); the user's next send hands it over.
  const run = useAction((name: string) => draftSkill(api, navigate, session, name))
  // A repo dir first (only removed when it holds `.git`), else a local skill, as the TUI.
  const remove = useAction(
    (s: SkillMeta) => api.remove_skill_repo(s.repo ?? s.name).catch(() => api.remove_skill(s.name)),
    { keys: [queryKeys.skills], onSuccess: () => setShown(null) },
  )

  return (
    <Section
      title="Skills"
      actions={
        <Button
          variant="ghost"
          size="sm"
          disabled={sync.isPending}
          onClick={() => sync.mutate(undefined)}
        >
          <RefreshCw className={sync.isPending ? 'animate-spin' : undefined} />
          Sync
        </Button>
      }
    >
      <form
        className="flex gap-2 pb-3"
        onSubmit={(e) => {
          e.preventDefault()
          if (url.trim()) addRepo.mutate(url.trim())
        }}
      >
        <Input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="Git URL of a skills repo"
          spellCheck={false}
        />
        <Button
          type="submit"
          variant="outline"
          disabled={url.trim().length === 0 || addRepo.isPending}
        >
          Add repo
        </Button>
      </form>
      {skills.data?.length === 0 && <Empty>No skills yet.</Empty>}
      {skills.data?.map((s) => (
        <button
          key={s.name}
          type="button"
          onClick={() => setShown(s)}
          className="flex w-full items-center gap-3 border-b border-border-subtle py-3 text-left last:border-0 hover:bg-layer-2/60"
        >
          <span
            className="w-3 shrink-0 text-center text-fg-accent"
            title={loaded?.includes(s.name) ? 'In this chat' : undefined}
          >
            {loaded?.includes(s.name) ? '●' : ''}
          </span>
          <span className="shrink-0 text-sm font-medium">{s.name}</span>
          <span className="min-w-0 flex-1 truncate text-[13px] text-fg-secondary">
            {s.description}
          </span>
          {s.repo && (
            <Badge variant="outline" className="shrink-0 font-normal">
              {s.repo}
            </Badge>
          )}
          {s.always && <Badge variant="secondary">always</Badge>}
        </button>
      ))}
      <Sheet open={shown !== null} onOpenChange={(open) => !open && setShown(null)}>
        <SheetContent className="flex w-full flex-col sm:w-[560px] sm:max-w-[560px]">
          {shown && (
            <>
              <SheetHeader>
                <SheetTitle>{shown.name}</SheetTitle>
                <SheetDescription>{shown.description}</SheetDescription>
              </SheetHeader>
              <SkillBody name={shown.name} />
              <SheetFooter className="flex-row justify-end">
                <Button
                  variant="outline"
                  disabled={remove.isPending}
                  onClick={() => remove.mutate(shown)}
                >
                  Remove
                </Button>
                <Button disabled={run.isPending} onClick={() => run.mutate(shown.name)}>
                  Use
                </Button>
              </SheetFooter>
            </>
          )}
        </SheetContent>
      </Sheet>
    </Section>
  )
}

/** A skill's markdown, rendered; shared with the transcript's skill card. Leading YAML front matter (name/description,
 * shown in the header already) is dropped. */
export function SkillBody({ name }: { name: string }) {
  const api = useApi()
  const body = useQuery({
    queryKey: queryKeys.skill(name),
    queryFn: () => api.load_skill(name),
  })
  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-4 text-sm">
      {body.isError && <p className="text-danger">{errorText(body.error)}</p>}
      {body.data && (
        <>
          <Streamdown mode="static">{body.data.body.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '')}</Streamdown>
          {body.data.files.length > 0 && (
            <p className="mt-4 text-[13px] text-fg-tertiary">Files: {body.data.files.join(', ')}</p>
          )}
        </>
      )}
    </div>
  )
}
