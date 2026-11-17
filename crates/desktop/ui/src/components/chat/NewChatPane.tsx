import { Capability, Unavailable, useSupported, useQuerySupport } from '@/rpc/capabilities'
import { privateRequirements, endlessRequirements } from './requirements'
import { useQuery } from '@tanstack/react-query'
import { type UseNavigateResult, useNavigate } from '@tanstack/react-router'
import { Check, ChevronDown, ChevronRight, Folder } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { Composer, type DraftCreate } from '@/components/chat/Composer'
import { Header } from '@/components/chat/Header'
import { LinkBanner } from '@/components/link/LinkFailure'
import { ProjectPicker } from '@/components/chat/ProjectPicker'
import { age } from '@/components/tasks/helpers'
import { useMesh } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { SidebarMenuAction } from '@/components/ui/sidebar'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useDraftTarget, useDrafts } from '@/lib/drafts'
import { draftProject, newDraft, usePanes } from '@/lib/panes'
import { useCompactorLabel } from '@/lib/endless'
import { VIEW, usePrivate } from '@/lib/private'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { EndlessConfig } from "@/rpc/bindings/method.agent.output"
import type { EngineClass } from "@/rpc/bindings/method.nodes.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import { tilde } from './HarnessCards'

/** 183 §12.5 / mock panel 5: one machine a new chat can start on; `id` `null` is this machine (the default). `older`: a
 * pre-183 engine (its stats carry no `os_name`), which cannot take a chat started from here. */
export type MachineRow = { id: string | null; name: string; detail: string; status: string; online: boolean; older: boolean; repos: string[] }

/** U3: why an `older` machine cannot be picked. */
export const olderText = (name: string): string => `${name} runs an older theuth; update it to start chats there`

/** Machine class words (`EngineStats.class`) for `os_name · class` lines here and in Settings › Network. */
export const CLASS: Record<EngineClass, string> = { laptop: 'laptop', desktop: 'desktop', server: 'server', container: 'container', vm: 'VM' }

/** This machine first, then every paired engine: `os_name · class` from its replicated stats; `LAN` / `via relay` /
 * `offline 3h` (an offline one cannot be picked). No peers, no rows: a lone engine shows no picker. */
export function machineRows(mesh: Pick<MeshStatus, 'self_id' | 'self_name' | 'peers' | 'stats'> | undefined, now = Date.now()): MachineRow[] {
  if (!mesh?.peers.some((p) => p.id !== mesh.self_id)) return []
  const facts = (id: string) => {
    const s = mesh.stats.find((x) => x.id === id)
    return { detail: s ? [s.os_name, CLASS[s.class]].filter(Boolean).join(' · ') : '', older: s?.os_name === '', repos: s?.repos ?? [] }
  }
  return [
    { id: null, name: mesh.self_name || 'This machine', status: 'this machine', online: true, ...facts(mesh.self_id), older: false },
    ...mesh.peers
      .filter((p) => p.id !== mesh.self_id)
      .map((p) => ({
        id: p.id,
        name: p.name || p.id.slice(0, 8),
        online: p.reachable,
        status: !p.reachable ? (p.last_seen_ms > 0 ? `offline ${age(p.last_seen_ms, now)}` : 'offline') : p.path === 'relay' ? 'via relay' : p.path === 'lan' ? 'LAN' : '',
        ...facts(p.id),
      })),
  ]
}

const COMPLETE_DEBOUNCE_MS = 250

/** 183 R1: the draft's target machine is no longer in the (loaded) mesh roster. */
export const vanishedTarget = (rows: readonly MachineRow[], loaded: boolean, engine: string | null): boolean =>
  loaded && engine !== null && !rows.some((r) => r.id === engine)

/** 183 R1: pick the new chat's machine; a different machine has other models, so the draft's model / effort pick is dropped with it. */
export function pickMachine(engine: string | null): void {
  const target = useDraftTarget.getState()
  if (target.engine !== engine) {
    const draft = useDrafts.getState()
    draft.resetPick()
    if (draft.settings.create?.advisor) draft.setCreate({ advisor: null })
  }
  target.set({ engine, cwd: null })
}

/** "Browse <machine>…": the folder typed with that machine's own path completion (`path_complete(…, engine)`). */
function BrowseDialog({ machine, open, onOpenChange, onPick }: { machine: MachineRow; open: boolean; onOpenChange(o: boolean): void; onPick(cwd: string): void }) {
  const canQuery1 = useQuerySupport(["path_complete"])
  const api = useApi()
  const [path, setPath] = useState('~/')
  // 183 R1: every keystroke is a forwarded call to the other machine; ask once typing pauses.
  const [asked, setAsked] = useState(path)
  useEffect(() => {
    const t = setTimeout(() => setAsked(path), COMPLETE_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [path])
  const { data } = useQuery({ queryKey: ['path-complete', machine.id, asked], queryFn: () => api.path_complete(null, asked, asked.length, machine.id), enabled: canQuery1 && (open) })
  const shown = asked === path ? data : undefined
  const accept = () => {
    if (!path.trim()) return
    onPick(path.trim())
    onOpenChange(false)
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Folder on {machine.name}</DialogTitle>
        </DialogHeader>
        <Input
          value={path}
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && accept()}
          className="font-mono"
          aria-label={`Folder on ${machine.name}`}
          autoFocus
        />
        <div className="max-h-60 overflow-y-auto">
          {/* The candidates' token span is for the text that was asked; a path typed since has none yet. */}
          {shown?.candidates
            .filter((c) => c.is_dir)
            .map((c) => (
              <button
                key={c.insert}
                type="button"
                className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left font-mono text-sm hover:bg-accent"
                onClick={() => setPath(path.slice(0, shown.token_start) + c.insert + path.slice(shown.token_end))}
              >
                <Folder className="size-4 shrink-0 text-fg-secondary" aria-hidden="true" />
                {c.display}
              </button>
            ))}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!path.trim()} onClick={accept}>
            Use this folder
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/**
 * 183 §12.5: `New chat on [machine ▾]` and the folder there — the target's repos, its default, or "Browse <machine>…".
 * The pick lives in `useDraftTarget` until the first send's `create_session` spends it. A private chat stays here.
 */
export function MachineBar({ priv }: { priv: boolean }) {
  const { data: mesh } = useMesh()
  const rows = machineRows(mesh)
  const { engine, cwd, set } = useDraftTarget()
  const [browse, setBrowse] = useState(false)
  const chosen = rows.find((r) => r.id === engine) ?? rows[0]
  // 183 R1: a target that left the mesh (unpaired, or the last peer gone) would fail the first send: back to this machine.
  const vanished = vanishedTarget(rows, mesh !== undefined, engine)
  useEffect(() => {
    if (vanished) pickMachine(null)
  }, [vanished])
  if (!chosen) return null
  return (
    <div className="mx-auto flex w-full max-w-[768px] flex-wrap items-center gap-2 px-4 pt-2 text-sm">
      <span className="font-medium">New chat on</span>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" disabled={priv} title={priv ? 'A private chat runs on this machine' : undefined} aria-label={`Machine: ${chosen.name}`}>
            {chosen.name}
            <ChevronDown className="size-4" aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-[min(21rem,calc(100vw-2rem))]">
          {rows.map((r) => (
            <DropdownMenuItem key={r.id ?? 'self'} disabled={!r.online || r.older} onSelect={() => pickMachine(r.id)} className="items-start gap-2.5">
              <span className={cn('mt-1.5 size-1.5 shrink-0 rounded-full', r.online ? 'bg-success' : 'ring-1 ring-fg-tertiary ring-inset')} aria-hidden="true" />
              <span className="min-w-0 flex-1">
                <span className="block font-medium">{r.name}</span>
                {r.detail && <span className="block text-xs text-fg-secondary">{r.detail}</span>}
                {r.older && <span className="block text-xs text-warning">{olderText(r.name)}</span>}
              </span>
              <span className={cn('flex shrink-0 items-center gap-1 text-xs text-fg-tertiary', r.status === 'via relay' && 'rounded-full border border-border px-2')}>
                {r.status}
                {r === chosen && <Check className="size-3.5" aria-hidden="true" />}
              </span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="sm" className="min-w-0 text-fg-secondary" aria-label={`Folder: ${cwd ? tilde(cwd) : 'default folder'}`}>
            <Folder className="size-4 shrink-0" aria-hidden="true" />
            <span className="truncate font-mono text-xs">{cwd ? tilde(cwd) : 'default folder'}</span>
            <ChevronDown className="size-4 shrink-0" aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-[min(21rem,calc(100vw-2rem))]">
          <DropdownMenuLabel>Folder on {chosen.name}</DropdownMenuLabel>
          <DropdownMenuItem onSelect={() => set({ cwd: null })}>
            <span className="flex-1">Default folder</span>
            {cwd === null && <Check className="size-3.5" aria-hidden="true" />}
          </DropdownMenuItem>
          {chosen.repos.map((r) => (
            <DropdownMenuItem key={r} onSelect={() => set({ cwd: r })}>
              <Folder className="size-4 text-fg-secondary" aria-hidden="true" />
              <span className="min-w-0 flex-1 truncate font-mono text-[13px]">{tilde(r)}</span>
              {cwd === r && <Check className="size-3.5" aria-hidden="true" />}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => setBrowse(true)} className="text-fg-secondary">
            Browse {chosen.name}…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {browse && <BrowseDialog machine={chosen} open onOpenChange={setBrowse} onPick={(c) => set({ cwd: c })} />}
      {chosen.older && <span role="alert" className="w-full text-xs text-warning">{olderText(chosen.name)}</span>}
    </div>
  )
}

/**
 * The new-chat body shared by the `/new` route and a draft pane: header, prompt, composer, create options.
 * Nothing exists until the first send's `new_session` / `create_session` reply (`openDraftSession`) names the session.
 */
export function NewChat({
  preset,
  pane,
  first = true,
  onClose,
  onCreated,
  onUncreated,
}: {
  /** `?project=<label>` from the sidebar's "New chat in <project>" (51 §3); followed when it changes. */
  preset?: string
  pane?: string
  first?: boolean
  onClose?(): void
  onCreated?(session: string): void
  /** 183 §5.6: a chat just created on another machine did not take its first message: back to this draft. */
  onUncreated?(session: string): void
}) {
  const canQuery2 = useSupported(endlessRequirements)
  const privateSupported = useSupported(privateRequirements)
  // 137 §11.3: the ∞ switch starts at `agent.endless.default_chat` and is remembered per draft only.
  const api = useApi()
  const create = useDrafts((s) => s.settings.create)
  const setCreate = useDrafts((s) => s.setCreate)
  const priv = create?.retention === 'private'
  const project = create?.project ?? null
  const lastPreset = useRef(preset)
  useEffect(() => {
    if (preset !== undefined && (lastPreset.current !== preset || useDrafts.getState().settings.create?.project === undefined))
      useDrafts.getState().setCreate({ project: preset })
    lastPreset.current = preset
  }, [preset])
  const endlessSettings = useQuery({ enabled: canQuery2, queryKey: queryKeys.endlessSettings, queryFn: () => api.endless_settings_get() })
  const endlessOn = create?.memory ? create.memory === 'endless' : canQuery2 && (endlessSettings.data?.default_chat ?? false)
  const draft: DraftCreate = { private: priv, project: project?.trim() || null, endless: endlessOn }
  return (
    <>
      <Header session={null} first={first} onClose={onClose} />
      <LinkBanner />
      <MachineBar priv={priv} />
      <div className="flex min-h-0 flex-1 flex-col justify-center pb-24">
        <h1 className="mb-6 text-center text-[28px] leading-9 font-semibold">Where should we begin?</h1>
        {(priv && !privateSupported) || (endlessOn && !canQuery2) ? <Unavailable /> : <Composer session={null} pane={pane} draftCreate={draft} onCreated={onCreated} onUncreated={onUncreated} />}
        <CreateOptions
          priv={priv}
          onPriv={(v) => {
            setCreate({ retention: v ? 'private' : 'durable', view: v ? VIEW : null, ...(v ? { advisor: null } : {}) })
            // A private chat's tree is owned by this window's view (51 §4.3): it starts here.
            if (v) useDraftTarget.getState().set({ engine: null, cwd: null })
          }}
          project={project}
          onProject={(project) => setCreate({ project })}
          endless={endlessOn}
          onEndless={(on) => setCreate({ memory: on ? 'endless' : 'epochs' })}
          settings={endlessSettings.data}
        />
      </div>
    </>
  )
}

/**
 * The sidebar's New chat, ⇧⌘O and the project header's "New chat in <project>" (25 §12.3): with the view split, the
 * focused pane becomes a draft and the URL stays put; a single pane goes to the whole-window `/new`.
 */
export function openNewChat(navigate: UseNavigateResult<string>, project?: string | null): void {
  const panes = usePanes.getState()
  const draft = panes.fresh(project)
  if (!draft) void navigate({ to: '/new', search: project ? { project } : {} })
  // A new draft's pane takes the caret on mount; a kept one has to be told.
  else if (draft === panes.focused) window.dispatchEvent(new Event('theuth:focus-composer'))
}

/** 183 §12.5: a new chat with its machine already picked (the sidebar's "New chat on ▸", ⌘K's entries). */
export function openNewChatOn(navigate: UseNavigateResult<string>, engine: string | null): void {
  pickMachine(engine)
  openNewChat(navigate)
}

/** 183 §12.5: the sidebar Chat row's ▸ — "New chat on" each machine; only with paired peers, like `MachineBar`. */
export function NewChatOnMenu() {
  const navigate = useNavigate()
  const rows = machineRows(useMesh().data)
  if (rows.length === 0) return null
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <SidebarMenuAction showOnHover aria-label="New chat on…" title="New chat on…">
          <ChevronRight />
        </SidebarMenuAction>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="right" align="start" className="w-[min(18rem,calc(100vw-2rem))]">
        <DropdownMenuLabel>New chat on</DropdownMenuLabel>
        {rows.map((r) => (
          <DropdownMenuItem key={r.id ?? 'self'} disabled={!r.online || r.older} title={r.older ? olderText(r.name) : undefined} onSelect={() => openNewChatOn(navigate, r.id)}>
            <span className="min-w-0 flex-1 truncate">{r.name}</span>
            <span className="text-xs text-fg-tertiary">{r.older ? 'needs update' : r.status}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/**
 * A split pane holding a New chat opened from the sidebar (25 §12.3). `id` is the draft leaf's client id; the
 * URL stays on the previously focused chat, and the first send swaps the created session into this leaf.
 */
export function NewChatPane({ id, first }: { id: string; first: boolean }) {
  const navigate = useNavigate()
  const focused = usePanes((s) => s.focused === id)
  useEffect(() => {
    if (focused) document.title = ''
  }, [focused])
  // Fresh from a drop: the user types at once. A remount while another pane is focused must not steal the caret.
  useEffect(() => {
    if (usePanes.getState().focused === id) window.dispatchEvent(new Event('theuth:focus-composer'))
  }, [id])
  const close = () => {
    const to = usePanes.getState().close(id)
    if (to) void navigate({ to: '/c/$session', params: { session: to } })
  }
  const created = (session: string) => {
    usePanes.getState().place(id, session, 'center')
    void navigate({ to: '/c/$session', params: { session } })
  }
  // 183 §5.6: the chat (opened at once) did not take its first message: its leaf becomes a draft again; the draft's
  // text was never acked, so it is still there.
  const uncreated = (session: string) => usePanes.getState().place(session, newDraft(draftProject(id)), 'center')
  return <NewChat preset={draftProject(id)} pane={id} first={first} onClose={close} onCreated={created} onUncreated={uncreated} />
}

/**
 * Private toggle, project picker, the ∞ (endless memory) toggle, and their notices (51 §1, §8.3; 137 §11.3); sits under
 * the composer. `settings` is `agent.endless` for the compactor line; absent before its first read.
 */
export function CreateOptions({
  priv,
  onPriv,
  project,
  onProject,
  endless,
  onEndless,
  settings,
}: {
  priv: boolean
  onPriv(v: boolean): void
  project: string | null
  onProject(v: string | null): void
  endless: boolean
  onEndless(v: boolean): void
  settings: EndlessConfig | undefined
}) {
  const acknowledged = usePrivate((s) => s.acknowledged)
  const acknowledge = usePrivate((s) => s.acknowledge)
  const opening = useDrafts((s) => s.opening)
  // Two draft panes may be open at once; label ids must not collide.
  const pid = useId()
  const supported = useSupported(endlessRequirements)
  const compactor = useCompactorLabel(settings?.model, endless && supported)
  return (
    <div className="mx-auto mt-1 w-full max-w-[768px] px-4 text-sm">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
        <Capability required={privateRequirements} hide><label className="flex items-center gap-2">
          <Switch checked={priv} disabled={opening !== null} onCheckedChange={onPriv} aria-label="Private session" />
          <span>Private</span>
        </label></Capability>
        <Capability required={endlessRequirements} hide><Tooltip>
          <TooltipTrigger asChild>
            <label className="flex items-center gap-2">
              <Switch checked={endless} onCheckedChange={onEndless} aria-label="Fractal context" />
              {/* One text size up, pulled up a hair: the glyph's optical centre sits on the x-height, not the baseline. */}
              <span aria-hidden className="relative -top-px text-base leading-5 [font-feature-settings:normal]">
                ∞
              </span>
            </label>
          </TooltipTrigger>
          <TooltipContent side="bottom">Fractal context</TooltipContent>
        </Tooltip></Capability>
        <div className="flex items-center gap-2">
          <Label htmlFor={pid} className="text-fg-secondary">
            Project
          </Label>
          <ProjectPicker id={pid} value={project} onChange={onProject} />
        </div>
      </div>
      {endless && (
        <div className="mt-3 rounded-xl border border-border-subtle bg-muted/40 p-3">
          <p className="font-medium">Fractal context</p>
          <ul className="mt-1 list-disc space-y-0.5 pl-5 text-fg-secondary">
            <li>Every turn starts from a compressed view of the whole chat; nothing is deleted, the distant past just gets coarser.</li>
            <li>A background summariser keeps the view fresh; the first answer after a long turn may wait a few seconds for it.</li>
            <li>zoom and recall bring any past message back word for word.</li>
          </ul>
          <p className="mt-2 text-xs text-fg-tertiary">Compactor: {compactor}</p>
        </div>
      )}
      {priv && (
        <div className="mt-3 rounded-xl border border-warning-border/40 bg-warning-bg p-3">
          <p className="font-medium">Private session</p>
          <ul className="mt-1 list-disc space-y-0.5 pl-5 text-fg-secondary">
            <li>theuth keeps this conversation, its artifacts and cost records in memory only; leaving or closing it deletes them.</li>
            <li>Scratch files it writes are real files in a temporary directory, removed on close (best effort: a crash or snapshot may keep them).</li>
            <li>This is not a sandbox: what a tool explicitly writes, sends or runs outside that directory is not undone.</li>
            <li>Model providers still see the request; their retention is not controlled by theuth.</li>
            <li>A reload or a lost connection longer than 30 s closes the session.</li>
          </ul>
          <label className="mt-2 flex items-center gap-2">
            <input
              type="checkbox"
              className="size-4"
              checked={acknowledged}
              disabled={acknowledged}
              onChange={(e) => e.target.checked && acknowledge()}
            />
            <span>I understand</span>
          </label>
        </div>
      )}
    </div>
  )
}
