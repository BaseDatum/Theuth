import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Archive, ArchiveRestore, ListChecks, MessageSquare, Network, Palette, StickyNote, Trash2 } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { useNotes } from '@/components/notes/useNotes'
import { ProjectIcon, ProjectStyleDialog } from '@/components/sidebar/ProjectStyle'
import { useTasks } from '@/components/tasks/queries'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { Project } from '@/rpc/bindings/method.agent.output'
import { DeleteProjectDialog } from './DeleteProjectDialog'
import { MetadataEditor, type Pair, sameMetadata, toMetadata, toPairs } from './MetadataEditor'
import { type ProjectCounts, useLiveProjects, useProject, useUpdateProject } from './queries'

/**
 * 223 `/projects/$name`: one project — description and metadata editors, icon & colour, archive, delete — and what
 * it groups: its task board, notes and chats. An implicit label shows the same page; the first save creates the row.
 */
export function ProjectDetail({ name }: { name: string }) {
  const navigate = useNavigate()
  const { project, isPending, error } = useProject(name)
  const update = useUpdateProject(name)
  useLiveProjects()
  const can = useQuerySupport(['list_sessions'])
  const api = useApi()
  const sessions = useQuery({ enabled: can, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const tasks = useTasks().data?.filter((t) => t.project === name) ?? []
  const notes = useNotes().data?.filter((d) => d.project === name) ?? []
  const chats = sessions.data?.filter((s) => !s.parent && s.project === name) ?? []
  const counts: ProjectCounts = { chats: chats.length, tasks: tasks.length, notes: notes.length }
  const [styling, setStyling] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const archived = project?.status === 'archived'

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="projects" />
      <SectionRow
        back={{ to: '/projects', label: 'All projects' }}
        title={
          <span className="flex min-w-0 items-center gap-2">
            <ProjectIcon project={name} className="size-4" />
            <span className="truncate">{name}</span>
            {archived && <Badge variant="outline">Archived</Badge>}
          </span>
        }
      >
        <Button variant="ghost" size="sm" className="ml-auto" disabled={!project} onClick={() => setStyling(true)}>
          <Palette /> Icon & colour
        </Button>
        <Button variant="ghost" size="sm" disabled={!project || update.isPending} onClick={() => update.mutate({ status: archived ? 'active' : 'archived' })}>
          {archived ? <ArchiveRestore /> : <Archive />} {archived ? 'Unarchive' : 'Archive'}
        </Button>
        <Button variant="ghost" size="sm" className="text-danger" disabled={!project} onClick={() => setDeleting(true)}>
          <Trash2 /> Delete
        </Button>
      </SectionRow>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-[768px] flex-col gap-6 p-4">
          {isPending && <Skeleton className="h-24 w-full" />}
          {error && <p className="text-sm text-danger">{error.message}</p>}
          {!isPending && !error && !project && <p className="text-sm text-fg-tertiary">No project named {name}.</p>}
          {project && (
            <>
              {project.implicit && (
                <p className="rounded-md border border-border-subtle p-3 text-xs text-fg-secondary">
                  Found on chats, tasks or notes but not created as a project yet. Saving a description or metadata creates it.
                </p>
              )}
              <Description key={project.description ?? ''} project={project} />
              <Metadata key={JSON.stringify(project.metadata)} project={project} />
            </>
          )}
          <Section icon={<ListChecks />} title="Tasks" count={tasks.length}>
            <p className="text-sm text-fg-secondary">
              {tasks.filter((t) => t.state !== 'done' && t.state !== 'cancelled').length} open of {tasks.length}.
            </p>
            <div className="flex gap-2">
              <Button asChild variant="outline" size="sm">
                <Link to="/tasks" search={{ project: name }}>
                  <ListChecks /> Task board
                </Link>
              </Button>
              <Button asChild variant="ghost" size="sm">
                <Link to="/tasks/graph" search={{ project: name }}>
                  <Network /> Graph
                </Link>
              </Button>
            </div>
          </Section>
          <Section icon={<StickyNote />} title="Notes" count={notes.length}>
            <ul>
              {notes.map((d) => (
                <li key={d.id}>
                  <Link to="/notes/$id" params={{ id: d.id }} className="block truncate rounded-md px-2 py-1.5 text-sm hover:bg-layer-2">
                    {d.title || 'Untitled'}
                  </Link>
                </li>
              ))}
            </ul>
          </Section>
          <Section icon={<MessageSquare />} title="Chats" count={chats.length}>
            <ul>
              {chats.map((s) => (
                <li key={s.id}>
                  <Link to="/c/$session" params={{ session: s.id }} className="block truncate rounded-md px-2 py-1.5 text-sm hover:bg-layer-2">
                    {s.title || 'Untitled chat'}
                  </Link>
                </li>
              ))}
            </ul>
          </Section>
        </div>
      </div>
      {project && <ProjectStyleDialog project={name} open={styling} onOpenChange={setStyling} />}
      <DeleteProjectDialog name={name} counts={counts} open={deleting} onOpenChange={setDeleting} onDeleted={() => void navigate({ to: '/projects' })} />
    </article>
  )
}

function Section({ icon, title, count, children }: { icon: ReactNode; title: string; count: number; children: ReactNode }) {
  return (
    <section aria-label={title} className="flex flex-col gap-2">
      <h2 className="flex items-center gap-2 text-sm font-semibold [&>svg]:size-4 [&>svg]:text-fg-tertiary">
        {icon}
        {title}
        <span className="font-normal text-fg-tertiary">{count}</span>
      </h2>
      {children}
    </section>
  )
}

/** The description, saved on blur when it changed (`''` clears it). */
function Description({ project }: { project: Project }) {
  const update = useUpdateProject(project.name)
  const [text, setText] = useState(project.description ?? '')
  const save = () => {
    const next = text.trim()
    if (next !== (project.description ?? '')) update.mutate({ description: next })
  }
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor="project-description" className="text-sm font-semibold">
        Description
      </label>
      <Textarea
        id="project-description"
        rows={3}
        placeholder="What this project is for"
        value={text}
        disabled={update.isPending}
        onChange={(e) => setText(e.target.value)}
        onBlur={save}
      />
    </div>
  )
}

/** The metadata rows; Save sends the whole map, Revert drops the edits. */
function Metadata({ project }: { project: Project }) {
  const update = useUpdateProject(project.name)
  const [pairs, setPairs] = useState<Pair[]>(() => toPairs(project.metadata))
  const dirty = !sameMetadata(toMetadata(pairs), project.metadata ?? {}) || pairs.some((p) => !p.key.trim())
  return (
    <div className="flex flex-col gap-1.5">
      <h2 className="text-sm font-semibold">Metadata</h2>
      <MetadataEditor value={pairs} onChange={setPairs} disabled={update.isPending} />
      {dirty && (
        <div className="flex gap-2">
          <Button size="sm" disabled={update.isPending} onClick={() => update.mutate({ metadata: toMetadata(pairs) })}>
            Save metadata
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setPairs(toPairs(project.metadata))}>
            Revert
          </Button>
        </div>
      )}
    </div>
  )
}
