import { code } from '@streamdown/code'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import { ChevronLeft, Copy, History as HistoryIcon, PanelRight, Pencil, Trash2 } from 'lucide-react'
import { type ComponentType, type MouseEvent, type ReactNode, memo, useMemo, useState } from 'react'
import { toast } from 'sonner'
import { type Components, Streamdown } from 'streamdown'
import { Backlinks, Links } from '@/components/knowledge/Backlinks'
import { Editor } from '@/components/knowledge/Editor'
import { History } from '@/components/knowledge/History'
import { NewDocDialog } from '@/components/knowledge/NewDocDialog'
import { scaffold, stripFrontmatter } from '@/components/knowledge/frontmatter'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useIsMobile } from '@/hooks/use-mobile'
import { isMacDesktop } from '@/lib/hotkeys'
import { classify, resolveLink } from '@/lib/resolveLink'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Concept } from '@/rpc/bindings/Concept'
import type { Meta } from '@/rpc/bindings/Meta'
import { cn } from 'cn'

const route = getRouteApi('/knowledge/$')
const plugins = { code }

/** `/knowledge/$`: one concept; `?draft=` is a not-yet-written one opened straight in the editor. */
export function Document() {
  const { _splat: id = '' } = route.useParams()
  const { draft } = route.useSearch()
  return <Doc key={id} id={id} draft={draft} />
}

/** Keyed by id so editing/dialog state never leaks between concepts. */
function Doc({ id, draft }: { id: string; draft: string | undefined }) {
  const api = useApi()
  const navigate = useNavigate()
  const [editing, setEditing] = useState(false)
  const [missing, setMissing] = useState<{ id: string; title: string } | null>(null)
  const [railOpen, setRailOpen] = useState(true)
  const mobile = useIsMobile()
  const doc = useQuery({
    queryKey: queryKeys.knowledgeDoc(id),
    queryFn: () => api.knowledge_get(id),
    enabled: draft === undefined,
  })

  if (draft !== undefined) {
    return (
      <Frame id={id}>
        <Editor
          id={id}
          initial={scaffold(id, draft)}
          onSaved={() =>
            navigate({
              to: '/knowledge/$',
              params: { _splat: id },
              search: {},
              replace: true,
            })
          }
          onCancel={() => navigate({ to: '/knowledge' })}
        />
      </Frame>
    )
  }
  if (doc.isPending) {
    return (
      <Frame id={id}>
        <div className="mx-auto w-full max-w-[768px] space-y-3 p-6">
          <Skeleton className="h-8 w-1/2" />
          <Skeleton className="h-4 w-3/4" />
          <Skeleton className="h-4 w-2/3" />
        </div>
      </Frame>
    )
  }
  if (doc.isError) {
    return (
      <Frame id={id}>
        <div className="m-auto text-center text-sm">
          <p className="text-danger">{doc.error.message}</p>
          <Button
            variant="ghost"
            size="sm"
            className="mt-2"
            onClick={() => setMissing({ id, title: id })}
          >
            Create {id}
          </Button>
          <NewDocDialog
            open={missing !== null}
            onOpenChange={(o) => !o && setMissing(null)}
            initial={missing ?? undefined}
          />
        </div>
      </Frame>
    )
  }
  const concept = doc.data
  if (editing) {
    return (
      <Frame id={id}>
        <Editor
          id={id}
          initial={concept.text}
          onSaved={() => setEditing(false)}
          onCancel={() => setEditing(false)}
        />
      </Frame>
    )
  }
  const rail = (
    <>
      <Backlinks ids={concept.cited_by} />
      <Links
        links={concept.links}
        onMissing={(target) => setMissing({ id: target, title: target })}
      />
    </>
  )
  return (
    <Frame
      id={id}
      actions={
        <Actions
          concept={concept}
          railOpen={railOpen}
          onEdit={() => setEditing(true)}
          onToggleRail={() => setRailOpen((v) => !v)}
        />
      }
    >
      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1 overflow-y-auto">
          <div className="mx-auto w-full max-w-[768px] px-6 py-4">
            <Header meta={concept.meta} />
            <Body
              concept={concept}
              onMissing={(target, title) => setMissing({ id: target, title })}
            />
          </div>
          {/* Under `md` the links rail has no room beside the body; it follows the body instead, still toggled by the same action. */}
          {mobile && railOpen && (
            <section className="border-t border-border-subtle">{rail}</section>
          )}
        </div>
        {!mobile && railOpen && (
          <aside className="w-[260px] shrink-0 overflow-y-auto border-l border-border-subtle">
            {rail}
          </aside>
        )}
      </div>
      <NewDocDialog
        open={missing !== null}
        onOpenChange={(o) => !o && setMissing(null)}
        initial={missing ?? undefined}
      />
    </Frame>
  )
}

/** 52 px bar (id left, actions right) over the pane body. Under `md` a back chevron returns to the tree. */
function Frame({
  id,
  actions,
  children,
}: {
  id: string
  actions?: ReactNode
  children: ReactNode
}) {
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={isMacDesktop() || undefined}
        className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3"
      >
        <Button
          asChild
          variant="ghost"
          size="icon-sm"
          className="md:hidden"
          aria-label="Back to knowledge"
        >
          <Link to="/knowledge">
            <ChevronLeft />
          </Link>
        </Button>
        <code className="min-w-0 flex-1 truncate font-mono text-xs text-fg-tertiary">{id}</code>
        {actions}
      </header>
      {children}
    </article>
  )
}

function Actions({
  concept,
  railOpen,
  onEdit,
  onToggleRail,
}: {
  concept: Concept
  railOpen: boolean
  onEdit: () => void
  onToggleRail: () => void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const id = concept.meta.id
  const [historyOpen, setHistoryOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const del = useMutation({
    mutationFn: () => api.knowledge_delete(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeList })
      qc.removeQueries({ queryKey: queryKeys.knowledgeDoc(id) })
      toast(`Deleted ${id}`)
      navigate({ to: '/knowledge' })
    },
    onError: (e) => toast.error(e.message),
  })
  const cited = concept.cited_by.length
  return (
    <>
      <Action label="Edit" icon={Pencil} onClick={onEdit} />
      <Action label="History" icon={HistoryIcon} onClick={() => setHistoryOpen(true)} />
      <Action
        label="Copy id"
        icon={Copy}
        onClick={() =>
          navigator.clipboard.writeText(id).then(
            () => toast('Copied'),
            (e: unknown) => toast.error(String(e)),
          )
        }
      />
      <Action label="Delete" icon={Trash2} onClick={() => setDeleting(true)} />
      <Action
        label={railOpen ? 'Hide links' : 'Show links'}
        icon={PanelRight}
        onClick={onToggleRail}
        pressed={railOpen}
      />
      <History id={id} open={historyOpen} onOpenChange={setHistoryOpen} />
      <AlertDialog open={deleting} onOpenChange={setDeleting}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {id}?</AlertDialogTitle>
            <AlertDialogDescription>
              {cited === 0
                ? 'Nothing links to this document.'
                : `${cited} ${cited === 1 ? 'document links' : 'documents link'} here; those links will break.`}{' '}
              The file stays in history.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={del.isPending}
              onClick={() => del.mutate()}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

function Action({
  label,
  icon: Icon,
  onClick,
  pressed,
}: {
  label: string
  icon: ComponentType<{ className?: string }>
  onClick: () => void
  pressed?: boolean
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          className="size-11 rounded-xl"
          aria-label={label}
          aria-pressed={pressed}
          onClick={onClick}
        >
          <Icon className={cn('size-[18px]', pressed === false && 'text-fg-tertiary')} />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

const Header = memo(function Header({ meta }: { meta: Meta }) {
  const facts = [meta.scope, meta.source, meta.timestamp].filter((s): s is string => s !== null)
  return (
    <header className="mb-6 border-b border-border-subtle pb-4">
      <div className="flex flex-wrap items-center gap-2">
        <h1 className="text-[24px] leading-8 font-semibold">{meta.title}</h1>
        <Badge variant="outline">{meta.type}</Badge>
        {meta.tags.map((t) => (
          <Badge key={t} variant="secondary">
            {t}
          </Badge>
        ))}
      </div>
      {meta.description && <p className="mt-1 text-fg-secondary">{meta.description}</p>}
      {facts.length > 0 && <p className="mt-1 text-sm text-fg-tertiary">{facts.join(' · ')}</p>}
    </header>
  )
})

const RESERVED: Record<string, true> = { index: true, log: true }

/** The markdown body with okf link semantics: external → opener, internal → route, missing → create. */
const Body = memo(function Body({
  concept,
  onMissing,
}: {
  concept: Concept
  onMissing: (id: string, title: string) => void
}) {
  const navigate = useNavigate()
  const id = concept.meta.id
  const body = useMemo(() => stripFrontmatter(concept.text).body, [concept.text])
  const exists = useMemo<Record<string, boolean>>(
    () => Object.fromEntries(concept.links),
    [concept.links],
  )

  const components = useMemo<Components>(
    () => ({
      a: ({ href = '', children, node: _node, className, ...rest }) => {
        const target = resolveLink(id, href)
        const kind = classify(href)
        const internal = kind === 'absolute' || kind === 'relative'
        const inert = internal && (target === null || RESERVED[target] === true)
        const missing = target !== null && exists[target] === false
        return (
          <a
            href={href}
            {...rest}
            title={inert ? 'Not a document' : missing ? 'Missing: click to create' : undefined}
            className={cn(
              className,
              inert && 'cursor-default text-fg-tertiary no-underline',
              missing && 'text-danger underline decoration-dotted',
            )}
          >
            {children}
          </a>
        )
      },
    }),
    [id, exists],
  )

  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const a = e.target instanceof Element ? e.target.closest('a') : null
    const href = a?.getAttribute('href')
    if (!a || !href) return
    const kind = classify(href)
    if (kind === 'anchor') return
    e.preventDefault()
    if (kind === 'external') {
      openUrl(href).catch((err: unknown) => toast.error(String(err)))
      return
    }
    const target = resolveLink(id, href)
    if (target === null || RESERVED[target]) return
    if (exists[target] === false) onMissing(target, a.textContent?.trim() || target)
    else navigate({ to: '/knowledge/$', params: { _splat: target } })
  }

  return (
    <div onClickCapture={onClick}>
      <Streamdown
        mode="static"
        plugins={plugins}
        components={components}
        linkSafety={{ enabled: false }}
      >
        {body}
      </Streamdown>
    </div>
  )
})
