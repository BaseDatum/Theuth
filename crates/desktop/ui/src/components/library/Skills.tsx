import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener'
import { FolderGit2, Plus, RefreshCw, Search, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Streamdown } from 'streamdown'
import { draftSkill, sendRaw } from '@/components/chat/slash'
import { Page } from '@/components/library/Page'
import { LOCAL, groupSkills, isEditable, repoDir, splitHarnessCommands } from '@/components/library/grouping'
import { errorText, toastSync, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { SkillMeta } from "@/rpc/bindings/method.agent.output"
import { useSessionField, useSessions } from '@/store/sessions'

const WHAT = 'A skill is markdown the model pulls into context on demand: the system prompt lists every skill by name and description, the model loads the body when it needs it. Skills marked always are inlined into every prompt instead.'

/**
 * `/skills` and `/skills/$name`: every skill under `<data_dir>/skills/` (local files and cloned repos) with
 * search, the selected one rendered on the right (below the list on phones). `session` (`?session=`) is the
 * chat the page was opened from: its loaded skills are marked and `Use` drafts into it. For an ACP child (`agent`
 * set) the harness's own manifest (75 §3) is listed first: loaded skills, then its commands — each row sends its
 * command to the session as typed (`/skill:<name>`, `/<command>`; 141 D15a), bypassing theuth's `/skill:` parking.
 */
export function SkillsPage({ name, session }: { name: string | null; session: string | null }) {
  const canQuery1 = useQuerySupport(["list_skills"])
  const api = useApi()
  const navigate = useNavigate()
  const skills = useQuery({ enabled: canQuery1, queryKey: queryKeys.skills, queryFn: () => api.list_skills() })
  const loaded = useSessionField(session ?? '', 'skills')
  const agent = useSessionField(session ?? '', 'agent')
  const harnessCommands = useSessionField(session ?? '', 'harnessCommands')
  const sessionLoaded = useSessionField(session ?? '', 'loaded')
  // 141 R3 T12: opened cold (`/skills?session=…` as the first route) the slice is empty — the harness manifest
  // only rides `load_session`, which the chat view would otherwise be the first to call.
  useEffect(() => {
    if (!session || sessionLoaded) return
    api.load_session(session, null, 1).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      () => {},
    )
  }, [api, session, sessionLoaded])
  const [query, setQuery] = useState('')
  const [dialog, setDialog] = useState<'new' | 'repos' | null>(null)
  const all = skills.data ?? []
  const groups = groupSkills(all, query)
  const harness = agent && harnessCommands?.length ? splitHarnessCommands(harnessCommands, query) : null
  const harnessRows = harness ? harness.skills.length + harness.commands.length : 0
  // D15 (a): the harness's command, verbatim, then the chat — never `draftSkill` (that parks theuth's skill of the same name).
  const send = useAction(async (command: string) => {
    if (!session) return
    await sendRaw(api, session, `/${command}`)
    await navigate({ to: '/c/$session', params: { session } })
  })
  const shown = name ? all.find((s) => s.name === name) ?? null : null

  return (
    <Page
      back={name ? { to: '/skills', label: 'Back to skills' } : undefined}
      actions={
        <>
          <Button variant="ghost" size="sm" onClick={() => setDialog('repos')}>
            <FolderGit2 /> Repos
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setDialog('new')}>
            <Plus /> New skill
          </Button>
        </>
      }
    >
      <div className="flex min-h-0 flex-1">
        <aside className={cn('w-full shrink-0 flex-col overflow-y-auto border-r border-border-subtle md:flex md:w-[320px]', name ? 'hidden' : 'flex')}>
          <div className="relative p-2">
            <Search className="pointer-events-none absolute top-1/2 left-4 size-4 -translate-y-1/2 text-fg-tertiary" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === 'Escape' && setQuery('')}
              placeholder="Search skills"
              aria-label="Search skills"
              className="h-9 rounded-[10px] bg-layer-2 pl-8"
              spellCheck={false}
            />
          </div>
          {skills.isPending && (
            <div className="space-y-2 p-3">
              <Skeleton className="h-8 w-2/3" />
              <Skeleton className="h-8 w-1/2" />
            </div>
          )}
          {skills.isError && <p className="p-3 text-sm text-fg-tertiary">{errorText(skills.error)}</p>}
          {skills.data?.length === 0 && (
            <div className="space-y-3 p-3 text-sm text-fg-tertiary">
              <p>No skills yet.</p>
              <p>{WHAT}</p>
              <p>Write one with New skill, or add a git repo of skills under Repos.</p>
            </div>
          )}
          {all.length > 0 && groups.length === 0 && harnessRows === 0 && <p className="p-3 text-sm text-fg-tertiary">No skill matches “{query.trim()}”.</p>}
          {harness && harnessRows > 0 && (
            <section className="px-2 pb-2">
              <h3 className="px-2.5 pt-2 pb-1 text-xs font-medium text-fg-tertiary">
                Harness skills <span className="font-normal">· loaded by {agent}</span>
              </h3>
              <ul>
                {harness.skills.map((s) => (
                  <li key={s.name}>
                    <button type="button" className="flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left text-sm hover:bg-layer-2" title={`send /skill:${s.name} to ${agent}`} onClick={() => send.mutate(`skill:${s.name}`)}>
                      <span className="truncate font-medium">{s.name}</span>
                      <span className="truncate text-[13px] text-fg-secondary">{s.description}</span>
                    </button>
                  </li>
                ))}
              </ul>
              {harness.commands.length > 0 && (
                <>
                  <h3 className="px-2.5 pt-2 pb-1 text-xs font-medium text-fg-tertiary">Commands</h3>
                  <ul>
                    {harness.commands.map((c) => (
                      <li key={c.name}>
                        <button type="button" className="flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left text-sm hover:bg-layer-2" title={`send /${c.name} to ${agent}`} onClick={() => send.mutate(c.name)}>
                          <span className="flex items-center gap-2">
                            <span className="min-w-0 flex-1 truncate font-medium">{c.name}</span>
                            {c.hint && <span className="truncate text-xs text-fg-tertiary">{c.hint}</span>}
                          </span>
                          <span className="truncate text-[13px] text-fg-secondary">{c.description}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </section>
          )}
          {groups.map((g) => (
            <section key={g.source} className="px-2 pb-2">
              <h3 className="px-2.5 pt-2 pb-1 text-xs font-medium text-fg-tertiary">
                {g.source === LOCAL ? 'Local' : g.source}
              </h3>
              <ul>
                {g.skills.map((s) => (
                  <li key={s.name}>
                    <Link
                      to="/skills/$name"
                      params={{ name: s.name }}
                      search={session ? { session } : {}}
                      className={cn('flex flex-col gap-0.5 rounded-[10px] px-2.5 py-2 text-sm hover:bg-layer-2', name === s.name && 'bg-layer-2')}
                    >
                      <span className="flex items-center gap-2">
                        <span className="min-w-0 flex-1 truncate font-medium">{s.name}</span>
                        {loaded?.includes(s.name) && (
                          <span className="text-fg-accent" title="In this chat">
                            ●
                          </span>
                        )}
                        {s.always && <Badge variant="secondary">always</Badge>}
                      </span>
                      <span className="truncate text-[13px] text-fg-secondary">{s.description}</span>
                    </Link>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </aside>
        <main className={cn('min-w-0 flex-1 overflow-y-auto md:flex', name ? 'flex' : 'hidden')}>
          {shown ? (
            <SkillDetail skill={shown} session={session} />
          ) : name && skills.data ? (
            <p className="m-auto text-sm text-fg-tertiary">No skill named “{name}”.</p>
          ) : (
            <p className="m-auto hidden max-w-md px-6 text-center text-sm text-fg-tertiary md:block">
              {all.length > 0 ? 'Pick a skill.' : WHAT}
            </p>
          )}
        </main>
      </div>
      <NewSkillDialog open={dialog === 'new'} onClose={() => setDialog(null)} session={session} />
      <ReposDialog open={dialog === 'repos'} onClose={() => setDialog(null)} skills={all} />
    </Page>
  )
}

function SkillDetail({ skill, session }: { skill: SkillMeta; session: string | null }) {
  const canQuery2 = useQuerySupport(["load_skill"])
  const api = useApi()
  const navigate = useNavigate()
  const body = useQuery({ enabled: canQuery2, queryKey: queryKeys.skill(skill.name), queryFn: () => api.load_skill(skill.name) })
  const editable = isEditable(skill)
  const repo = repoDir(skill)
  const keys = [queryKeys.skills, queryKeys.skill(skill.name)]
  // Park the skill on the chat's composer (a new chat when none is open); the user's next send hands it over.
  const use = useAction((_: void) => draftSkill(api, navigate, session, skill.name))
  // `add_skill` rewrites `<name>.md` with the same body: the only way to flip `always` over the RPC.
  const always = useAction(
    (on: boolean) => api.add_skill(skill.name, skill.description, body.data?.body ?? '', on),
    { keys },
  )
  const remove = useAction(
    (_: void) => (skill.repo ? api.remove_skill_repo(skill.repo) : api.remove_skill(skill.name)),
    { keys, onSuccess: () => void navigate({ to: '/skills', search: session ? { session } : {} }) },
  )
  const reveal = useAction((path: string) => revealItemInDir(path))
  const open = useAction((dir: string) => openPath(dir))

  return (
    <div className="flex min-h-0 w-full flex-col">
      <div className="mx-auto w-full max-w-3xl px-6 py-4">
        <div className="flex flex-wrap items-start gap-2">
          <h3 className="min-w-0 flex-1 text-lg font-semibold">{skill.name}</h3>
          <Button size="sm" disabled={use.isPending} onClick={() => use.mutate(undefined)}>
            Use
          </Button>
          <Button variant="outline" size="sm" disabled={reveal.isPending} onClick={() => reveal.mutate(skill.path)}>
            Reveal file
          </Button>
          {repo && (
            <Button variant="outline" size="sm" disabled={open.isPending} onClick={() => open.mutate(repo)}>
              Open repo
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={skill.repo ? `Remove repo ${skill.repo}` : `Remove ${skill.name}`}
            title={skill.repo ? `Remove repo ${skill.repo}` : 'Remove skill'}
            disabled={remove.isPending}
            onClick={() =>
              confirm(skill.repo ? `Remove the repo ${skill.repo} and every skill in it?` : `Delete the skill ${skill.name}?`) &&
              remove.mutate(undefined)
            }
          >
            <Trash2 />
          </Button>
        </div>
        <p className="mt-1 text-sm text-fg-secondary">{skill.description}</p>
        <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-[13px]">
          <dt className="text-fg-tertiary">name</dt>
          <dd className="font-mono">{skill.name}</dd>
          <dt className="text-fg-tertiary">description</dt>
          <dd>{skill.description}</dd>
          <dt className="text-fg-tertiary">always</dt>
          <dd className="flex items-center gap-2">
            <Switch
              size="sm"
              checked={skill.always}
              disabled={!editable || !body.data || always.isPending}
              aria-label="Inline into every system prompt"
              onCheckedChange={(on) => always.mutate(on)}
            />
            <span className="text-fg-secondary">
              {skill.always ? 'inlined into every system prompt' : 'listed; loaded on demand'}
              {!editable && (skill.repo ? ' · change it upstream and sync' : ' · edit SKILL.md directly')}
            </span>
          </dd>
          <dt className="text-fg-tertiary">source</dt>
          <dd>{skill.repo ? <Badge variant="outline" className="font-normal">{skill.repo}</Badge> : 'local'}</dd>
          <dt className="text-fg-tertiary">path</dt>
          <dd className="break-all font-mono text-fg-secondary">{skill.path}</dd>
        </dl>
        <hr className="my-4 border-border-subtle" />
      </div>
      <SkillBody name={skill.name} />
    </div>
  )
}

/** A skill's markdown, rendered; shared with the transcript's skill card. Leading YAML front matter (name/description,
 * shown in the header already) is dropped. */
export function SkillBody({ name }: { name: string }) {
  const canQuery3 = useQuerySupport(["load_skill"])
  const api = useApi()
  const body = useQuery({ enabled: canQuery3,
    queryKey: queryKeys.skill(name),
    queryFn: () => api.load_skill(name),
  })
  return (
    <div className="mx-auto min-h-0 w-full max-w-3xl flex-1 overflow-y-auto px-6 pb-6 text-sm">
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

const EMPTY_SKILL = { name: '', description: '', body: '', always: false }

function NewSkillDialog({ open, onClose, session }: { open: boolean; onClose: () => void; session: string | null }) {
  const api = useApi()
  const navigate = useNavigate()
  const [form, setForm] = useState(EMPTY_SKILL)
  const add = useAction((v: typeof form) => api.add_skill(v.name.trim(), v.description.trim(), v.body, v.always), {
    keys: [queryKeys.skills],
    onSuccess: (s) => {
      setForm(EMPTY_SKILL)
      onClose()
      void navigate({ to: '/skills/$name', params: { name: s.name }, search: session ? { session } : {} })
    },
  })
  const ready = form.name.trim() && form.body.trim()
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-[640px]">
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (ready) add.mutate(form)
          }}
        >
          <DialogHeader>
            <DialogTitle>New skill</DialogTitle>
            <DialogDescription>Saved as a markdown file under skills/; the model sees it from its next call.</DialogDescription>
          </DialogHeader>
          <div className="grid gap-1.5">
            <Label htmlFor="skill-name">Name</Label>
            <Input
              id="skill-name"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="lowercase, digits, . _ -"
              spellCheck={false}
              autoFocus
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="skill-description">Description</Label>
            <Input
              id="skill-description"
              value={form.description}
              onChange={(e) => setForm({ ...form, description: e.target.value })}
              placeholder="One line the model reads to decide when to load it"
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="skill-body">Body</Label>
            <Textarea
              id="skill-body"
              value={form.body}
              onChange={(e) => setForm({ ...form, body: e.target.value })}
              placeholder="Markdown instructions"
              className="min-h-[220px] font-mono text-[13px]"
              spellCheck={false}
            />
          </div>
          <label className="flex items-center gap-2 text-sm">
            <Switch checked={form.always} onCheckedChange={(always) => setForm({ ...form, always })} />
            Always: inline into every system prompt instead of listing it
          </label>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={!ready || add.isPending}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function ReposDialog({ open, onClose, skills }: { open: boolean; onClose: () => void; skills: readonly SkillMeta[] }) {
  const api = useApi()
  const [url, setUrl] = useState('')
  // ponytail: repos are derived from the skills they hold; a clone with none does not show (no list_repos RPC).
  const repos = [...new Set(skills.flatMap((s) => (s.repo ? [s.repo] : [])))].sort()
  const addRepo = useAction((u: string) => api.add_skill_repo(u, null), {
    keys: [queryKeys.skills],
    onSuccess: (added) => {
      setUrl('')
      toast.success(`Added ${added.length} skills`)
    },
  })
  const sync = useAction((_: void) => api.sync_skills(), { keys: [queryKeys.skills], onSuccess: toastSync })
  const remove = useAction((repo: string) => api.remove_skill_repo(repo), { keys: [queryKeys.skills] })
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-[560px]">
        <DialogHeader>
          <DialogTitle>Skill repos</DialogTitle>
          <DialogDescription>Git repos cloned under skills/; every markdown file in them is a skill. Sync pulls them all.</DialogDescription>
        </DialogHeader>
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (url.trim()) addRepo.mutate(url.trim())
          }}
        >
          <Input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="Git URL of a skills repo" spellCheck={false} />
          <Button type="submit" variant="outline" disabled={url.trim().length === 0 || addRepo.isPending}>
            Add repo
          </Button>
        </form>
        {repos.length === 0 && <p className="text-sm text-fg-tertiary">No repos yet.</p>}
        <ul className="divide-y divide-border-subtle">
          {repos.map((r) => (
            <li key={r} className="flex items-center gap-3 py-2 text-sm">
              <span className="min-w-0 flex-1 truncate font-medium">{r}</span>
              <span className="text-[13px] text-fg-tertiary">{skills.filter((s) => s.repo === r).length} skills</span>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Remove repo ${r}`}
                disabled={remove.isPending}
                onClick={() => confirm(`Remove the repo ${r} and every skill in it?`) && remove.mutate(r)}
              >
                <Trash2 />
              </Button>
            </li>
          ))}
        </ul>
        <DialogFooter>
          <Button variant="outline" disabled={sync.isPending} onClick={() => sync.mutate(undefined)}>
            <RefreshCw className={sync.isPending ? 'animate-spin' : undefined} />
            Sync repos
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
