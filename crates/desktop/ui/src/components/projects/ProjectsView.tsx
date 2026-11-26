import { Link, useNavigate } from '@tanstack/react-router'
import { ListChecks, MoreHorizontal, Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import type { Project } from '@/rpc/bindings/method.agent.output'
import { DeleteProjectDialog } from './DeleteProjectDialog'
import { NewProjectDialog } from './NewProjectDialog'
import { type ProjectCounts, countsOf, itemsText, useLiveProjects, useProjectCounts, useProjectList } from './queries'

/** 223 `/projects`: every project (rows, then labels with no row as implicit), with what each groups; New project. */
export function ProjectsView() {
  const navigate = useNavigate()
  const list = useProjectList()
  const counts = useProjectCounts()
  const [creating, setCreating] = useState(false)
  useLiveProjects()
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="projects" />
      <SectionRow>
        <h1 className="text-sm font-semibold">Projects</h1>
        <Button variant="ghost" size="sm" className="ml-auto" onClick={() => setCreating(true)}>
          <Plus /> New project
        </Button>
      </SectionRow>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[768px] p-3">
          {list.isPending && (
            <div className="space-y-2 p-2">
              <Skeleton className="h-10 w-2/3" />
              <Skeleton className="h-10 w-1/2" />
            </div>
          )}
          {list.isError && <p className="p-2 text-sm text-danger">{list.error.message}</p>}
          {list.data?.length === 0 && (
            <p className="p-6 text-center text-sm text-fg-tertiary">
              No projects yet. A project groups chats, tasks and notes under one name. New project starts one.
            </p>
          )}
          <ul aria-label="Projects">
            {list.data?.map((p) => <Row key={p.name} project={p} counts={countsOf(counts, p.name)} />)}
          </ul>
        </div>
      </div>
      <NewProjectDialog open={creating} onOpenChange={setCreating} onCreated={(p) => void navigate({ to: '/projects/$name', params: { name: p.name } })} />
    </article>
  )
}

function Row({ project: p, counts }: { project: Project; counts: ProjectCounts }) {
  const [deleting, setDeleting] = useState(false)
  const items = itemsText(counts)
  return (
    <li className="group flex items-center gap-2 rounded-[10px] px-2.5 hover:bg-layer-2" data-project={p.name}>
      <ProjectIcon project={p.name} className="size-4" />
      <Link to="/projects/$name" params={{ name: p.name }} className="flex min-w-0 flex-1 flex-col py-2">
        <span className="flex min-w-0 items-center gap-2 text-sm">
          <span className="truncate">{p.name}</span>
          {p.status === 'archived' && <Badge variant="outline">Archived</Badge>}
          {p.implicit && (
            <Badge variant="outline" title="Found on chats, tasks or notes; not created as a project yet">
              Label only
            </Badge>
          )}
        </span>
        {p.description && <span className="truncate text-xs text-fg-secondary">{p.description}</span>}
      </Link>
      <span className="shrink-0 text-xs text-fg-tertiary">{items ?? 'Empty'}</span>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${p.name}`} className="opacity-0 group-hover:opacity-100 focus:opacity-100 data-[state=open]:opacity-100">
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem asChild>
            <Link to="/tasks" search={{ project: p.name }}>
              <ListChecks /> Task board
            </Link>
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onSelect={() => setDeleting(true)}>
            <Trash2 /> Delete…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <DeleteProjectDialog name={p.name} counts={counts} open={deleting} onOpenChange={setDeleting} />
    </li>
  )
}
