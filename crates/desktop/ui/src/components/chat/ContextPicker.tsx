import { Braces, ChevronRight, Pilcrow } from 'lucide-react'
import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { type Api, type ContextItem, type ContextSnapshot, type ContextTarget, contextItems, removeContext } from '@/lib/context-197'
import { routed } from '@/lib/owner'
import { privateOwner } from '@/lib/private'
import { cn } from '@/lib/utils'
import { useEngines } from '@/link/engines'
import { RpcError } from '@/rpc/client'
import { scope } from '@/store/engine-scope'
import { useSessions } from '@/store/sessions'
import { Img } from './Attachments'
import { type Category, type Group, type ShakeWord, type Tri, approx, catalog, expand, key, preselect, review, toggle, tokens, tri, wordKinds } from './shake'

type Request = {
  session: string
  word?: ShakeWord
  targets?: ContextTarget[]
  /** Revision of the history the button was rendered from; a differing inventory refuses rather than retargets. */
  expectedRevision?: string
}

/**
 * Open the shared context picker (48 §0.1): the composer's slash command, the session menu and the transcript's remove
 * buttons all end here. `targets` preselects exact server-issued targets and opens the review (a button, with the
 * displayed `expectedRevision`); `word` preselects its category (197 §7); bare opens the inventory unselected.
 */
export function openContextPicker(session: string, word?: ShakeWord, targets?: ContextTarget[], expectedRevision?: string): void {
  window.dispatchEvent(new CustomEvent<Request>('theuth:shake', { detail: { session, word, targets, expectedRevision } }))
}

const EMPTY: Record<ShakeWord, string> = {
  images: 'No images in current context',
  documents: 'No documents in current context',
  surfaces: 'No surfaces or diagrams in current context',
  tools: 'No tool exchanges in current context',
  notes: 'No harness notes in current context',
  agents: 'No agent messages in current context',
  messages: 'No messages in current context',
  summary: 'No summary in current context',
}

export const BUSY = 'Session is busy; wait or stop it, then retry.'

type Phase =
  | { at: 'loading' }
  | { at: 'pick' | 'review' | 'pending'; snap: ContextSnapshot; selected: Set<string> }
  | { at: 'error'; message: string; retry: boolean }

/**
 * Mounted once in `Composer`. Fetches the server inventory of the whole active context, lets the user pick by category,
 * group or item (197 §8), reviews the exact expanded set, then sends one `remove_context`. No optimistic transcript
 * surgery: success emits `theuth:reload-session` and a toast; failure keeps everything and never retries a destructive
 * RPC by itself.
 */
export function ContextPicker({ session }: { session: string }) {
  const [req, setReq] = useState<(Request & { api: Api; engine: string }) | null>(null)
  const [phase, setPhase] = useState<Phase>({ at: 'loading' })
  const [open, setOpen] = useState<Set<string>>(new Set())

  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<Request>).detail
      if (d.session !== session) return
      // Capture the initiating engine: a reply after an engine switch must not touch the other engine (48 §5).
      const engine = useEngines.getState().active
      const api = scope(engine).connection.getState().api
      if (!api) return toast.error('Not connected')
      setReq({ ...d, api, engine })
    }
    window.addEventListener('theuth:shake', on)
    return () => window.removeEventListener('theuth:shake', on)
  }, [session])

  // Inventory load; `req` identity guards a reply for a picker already closed or reopened.
  useEffect(() => {
    if (!req) return
    let live = true
    setPhase({ at: 'loading' })
    contextItems(req.api, req.session).then(
      (snap) => {
        if (!live) return
        if (!snap.editable) return setPhase({ at: 'error', message: snap.reason ?? BUSY, retry: true })
        // A button rendered from an older history must not retarget a reused seq: refuse, the reload refreshes it.
        if (req.expectedRevision !== undefined && req.expectedRevision !== snap.revision)
          return setPhase({ at: 'error', message: 'The context changed since this was shown. Reload the transcript and review again.', retry: false })
        const all = new Set(snap.items.map((i) => key(i.target)))
        if (req.targets) {
          const selected = new Set(req.targets.map(key).filter((k) => all.has(k)))
          // A button whose target left the context (already removed): nothing to preselect, so refuse rather than guess.
          if (selected.size === 0) return setPhase({ at: 'error', message: 'That item is no longer in the current context.', retry: false })
          return setPhase({ at: 'review', snap, selected })
        }
        const selected = req.word ? preselect(req.word, snap.items) : new Set<string>()
        if (req.word && req.word !== 'messages' && selected.size === 0) return setPhase({ at: 'error', message: EMPTY[req.word], retry: false })
        setOpen(new Set(req.word ? wordKinds(req.word) : []))
        setPhase({ at: 'pick', snap, selected })
      },
      (e: unknown) => live && setPhase({ at: 'error', message: e instanceof Error ? e.message : String(e), retry: true }),
    )
    return () => {
      live = false
    }
  }, [req])

  const close = () => phase.at !== 'pending' && setReq(null)

  const confirm = async () => {
    if (phase.at !== 'review' || !req) return
    const { snap, selected } = phase
    const { targets } = expand(snap.items, selected)
    // Local gate before the destructive call: the server rechecks, but a session that went busy, a slice marked stale
    // or a newer revision while the dialog sat open means the reviewed set no longer describes the context.
    const st = scope(req.engine)
    const slice = st.sessions.getState().sessions[req.session]
    if (st.connection.getState().state !== 'open') return setPhase({ at: 'error', message: 'Not connected. Review again once reconnected.', retry: true })
    if (slice?.running || slice?.contextStale) return setPhase({ at: 'error', message: BUSY, retry: true })
    if (slice?.contextRevision !== undefined && slice.contextRevision !== snap.revision)
      return setPhase({ at: 'error', message: 'The context changed while you were reviewing. Review again.', retry: true })
    setPhase({ at: 'pending', snap, selected })
    try {
      // 183 §4.4: the owner's call; unreachable, it waits behind the chat's card and the picker closes.
      const r = await routed(req.session, () => removeContext(req.api, req.session, snap.revision, targets))
      if (!r) return setReq(null)
      const parts = [
        r.messages > 0 && `${r.messages} message${r.messages === 1 ? '' : 's'}`,
        r.images > 0 && `${r.images} image${r.images === 1 ? '' : 's'}`,
        r.documents > 0 && `${r.documents} document${r.documents === 1 ? '' : 's'}`,
        r.surfaces > 0 && `${r.surfaces} surface${r.surfaces === 1 ? '' : 's'}`,
        r.heads > 0 && `${r.heads} summar${r.heads === 1 ? 'y' : 'ies'}`,
      ].filter((p): p is string => typeof p === 'string')
      toast(parts.length ? `Removed from context: ${parts.join(', ')} · ~${r.estimated_tokens} tokens now` : 'Nothing changed')
      // `ContextChanged` marks the slice stale; the settled reload here only targets the engine still on screen.
      if (useEngines.getState().active === req.engine)
        window.dispatchEvent(new CustomEvent('theuth:reload-session', { detail: { session: req.session, settled: true } }))
      setReq(null)
    } catch (e) {
      // A server refusal (stale, busy, malformed) wrote nothing; a transport failure has an unknown outcome. Either way:
      // show it, reload the inventory on request, never replay the RPC.
      const m = e instanceof Error ? e.message : String(e)
      const unknown = (e instanceof RpcError && e.code === -32300) || st.connection.getState().state !== 'open'
      setPhase({
        at: 'error',
        message: unknown ? `Connection lost; whether the removal committed is unknown. Review again before retrying. (${m})` : `Nothing was changed: ${m}`,
        retry: true,
      })
    }
  }

  const snap = phase.at === 'error' || phase.at === 'loading' ? null : phase.snap
  const cats = useMemo(() => (snap ? catalog(snap.items) : []), [snap])
  const selected = phase.at === 'error' || phase.at === 'loading' ? null : phase.selected
  const removing = useMemo(() => (snap && selected ? tokens(snap.items, selected) : 0), [snap, selected])
  const count = selected?.size ?? 0
  // Enter is the footer's default (197 §8) unless focus is on a control that has its own Enter.
  const primary = phase.at === 'pick' ? () => count > 0 && setPhase({ ...phase, at: 'review' }) : phase.at === 'review' ? () => void confirm() : undefined
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'Enter' || !primary || (e.target instanceof Element && e.target.closest('button,a,textarea'))) return
    e.preventDefault()
    primary()
  }

  return (
    <Dialog open={req !== null} onOpenChange={(o) => !o && close()}>
      <DialogContent
        className="flex max-h-[85vh] flex-col gap-0 p-0 sm:max-w-2xl"
        showCloseButton={phase.at !== 'pending'}
        onKeyDown={onKeyDown}
        onCloseAutoFocus={(e) => {
          // The opener may be a transcript button unmounted by the reload; the composer is always there.
          e.preventDefault()
          window.dispatchEvent(new Event('theuth:focus-composer'))
        }}
      >
        <DialogHeader className="border-b px-4 pt-4 pb-3 text-left">
          <DialogTitle>{phase.at === 'review' || phase.at === 'pending' ? 'Review' : 'Shake context'}</DialogTitle>
          <DialogDescription>
            {phase.at === 'loading' && 'Reading the active context…'}
            {phase.at === 'pick' && `Active context · ${phase.snap.items.length} items · ${approx(phase.snap.estimated_tokens)} tokens`}
            {(phase.at === 'review' || phase.at === 'pending') && "What leaves the model's context"}
            {phase.at === 'error' && 'Remove from context'}
          </DialogDescription>
        </DialogHeader>
        {phase.at === 'error' && <p className="px-4 py-3 text-sm">{phase.message}</p>}
        {phase.at === 'pick' &&
          (phase.snap.items.length === 0 ? (
            <p className="px-4 py-3 text-sm">Nothing in current context</p>
          ) : (
            <Pick
              session={session}
              cats={cats}
              selected={phase.selected}
              open={open}
              onOpen={setOpen}
              onChange={(s) => setPhase({ ...phase, selected: s })}
            />
          ))}
        {(phase.at === 'review' || phase.at === 'pending') && <ReviewBody cats={cats} items={phase.snap.items} selected={phase.selected} />}
        <DialogFooter className="flex-row flex-wrap items-center justify-end border-t px-4 py-3">
          {snap && (phase.at === 'pick' || phase.at === 'review' || phase.at === 'pending') && (
            <p className="basis-full text-sm sm:flex-1 sm:basis-auto" aria-live="polite">
              {count > 0 ? `${count} selected · ${approx(removing)} tokens · leaves ${approx(Math.max(0, snap.estimated_tokens - removing))}` : 'Tick a category, or open one to pick items'}
            </p>
          )}
          {phase.at === 'error' && phase.retry && (
            <Button variant="outline" onClick={() => req && setReq({ ...req })}>
              Review again
            </Button>
          )}
          {phase.at === 'review' && !req?.targets && (
            <Button variant="outline" onClick={() => setPhase({ ...phase, at: 'pick' })}>
              Back
            </Button>
          )}
          <Button variant="outline" disabled={phase.at === 'pending'} onClick={close}>
            Cancel
          </Button>
          {phase.at === 'pick' && phase.snap.items.length > 0 && (
            <Button disabled={count === 0} onClick={primary}>
              Review removal…
            </Button>
          )}
          {(phase.at === 'review' || phase.at === 'pending') && (
            <Button variant="destructive" disabled={phase.at === 'pending'} onClick={() => void confirm()}>
              {phase.at === 'pending' ? 'Removing…' : 'Remove from context'}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/** Items shown per group before "Show N more"; each press shows up to `PAGE` more, so a 2,000-item group never mounts at once. */
const FIRST = 4
const PAGE = 100

/** Native checkbox in a 44 px hit area; `some` is `indeterminate` (and `aria-checked="mixed"` for the DOM-less render). */
function Check({ state, label, onChange }: { state: Tri; label: string; onChange: () => void }) {
  return (
    <label className="-m-3 flex size-11 shrink-0 cursor-pointer items-center justify-center" onClick={(e) => e.stopPropagation()}>
      <input
        type="checkbox"
        data-nav=""
        className="size-4 cursor-pointer accent-foreground"
        aria-label={label}
        aria-checked={state === 'some' ? 'mixed' : undefined}
        checked={state === 'all'}
        ref={(el) => {
          if (el) el.indeterminate = state === 'some'
        }}
        onChange={onChange}
      />
    </label>
  )
}

/** A bar scaled to `max` and `~tokens`; the bar hides on a phone, the number stays. */
function Tokens({ value, max, color }: { value: number; max: number; color: string }) {
  return (
    <div className="flex w-14 items-center justify-end gap-2 text-xs tabular-nums sm:w-[120px]">
      <div className="hidden h-1.5 flex-1 overflow-hidden rounded-full bg-layer-2 sm:block">
        <i className="block h-full rounded-full" style={{ width: `${Math.max(3, Math.round((value / Math.max(1, max)) * 100))}%`, background: color }} />
      </div>
      <span>{approx(value)}</span>
    </div>
  )
}

const ROW = 'grid min-h-11 items-center gap-2.5 pr-2.5 hover:bg-layer-2 has-[:focus-visible]:bg-layer-2'

/** `HH:MM` of an RFC 3339 row time. */
const clock = (at: string | null | undefined) => (at ? new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : null)

/** Thumbnail from the loaded transcript's attachment path, else a format badge (an image's MIME subtype, a document's format). */
function Badge({ item, path, session }: { item: ContextItem; path?: string | null; session: string }) {
  if (item.kind === 'image' && path) return <Img path={path} session={privateOwner(session)} className="h-4 w-[22px] rounded-sm border object-cover" />
  if (item.kind === 'image' || item.kind === 'document')
    return <span className="w-[22px] truncate text-center font-mono text-[9px] text-fg-tertiary">{item.detail?.split(' ')[0] ?? (item.kind === 'image' ? 'IMG' : 'DOC')}</span>
  const Icon = item.kind === 'surface' || item.kind === 'tool' ? Braces : Pilcrow
  return <Icon className="mx-auto size-3.5 text-fg-tertiary" aria-hidden="true" />
}

/**
 * The picker (197 §8): two sections of category rows; an open category shows its group rows (when it has several) and
 * items, four per group then "Show N more". Conversation categories have no select-all (Sky's ruling). Arrow keys move
 * between the rows' controls; Space toggles a checkbox or a chevron.
 */
export function Pick({
  session,
  cats,
  selected,
  open,
  onOpen,
  onChange,
}: {
  session: string
  cats: Category[]
  selected: Set<string>
  open: Set<string>
  onOpen(o: Set<string>): void
  onChange(s: Set<string>): void
}) {
  const [shown, setShown] = useState<Record<string, number>>({})
  // Thumbnails come from the loaded transcript's occurrences (48 §3.3); an item outside the loaded page gets its badge.
  const entries = useSessions((s) => s.sessions[session]?.entries)
  const previews = useMemo(() => new Map((entries ?? []).flatMap((e) => (e.context?.attachments ?? []).map((a) => [key(a.target), a.path] as const))), [entries])
  const max = Math.max(1, ...cats.map((c) => c.tokens))
  const flip = (kind: string) => {
    const next = new Set(open)
    if (!next.delete(kind)) next.add(kind)
    onOpen(next)
  }
  const nav = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
    const all = [...e.currentTarget.querySelectorAll<HTMLElement>('[data-nav]')]
    const at = all.indexOf(document.activeElement as HTMLElement)
    all[Math.min(all.length - 1, Math.max(0, at + (e.key === 'ArrowDown' ? 1 : -1)))]?.focus()
    e.preventDefault()
  }
  const itemRow = (item: ContextItem) => {
    const k = key(item.target)
    const sub = [item.detail, clock(item.at)].filter(Boolean).join(' · ')
    return (
      <div key={k} className={cn(ROW, 'cursor-pointer grid-cols-[20px_22px_minmax(0,1fr)_auto] pl-10 sm:pl-[78px]')} onClick={() => onChange(toggle([k], selected))}>
        <Check state={selected.has(k) ? 'all' : 'none'} label={item.label} onChange={() => onChange(toggle([k], selected))} />
        <Badge item={item} path={previews.get(k)} session={session} />
        <span className="min-w-0">
          <span className="block truncate">{item.label}</span>
          {sub && <span className="block truncate text-xs text-fg-tertiary">{sub}</span>}
        </span>
        <span className="text-right text-xs tabular-nums text-fg-secondary">{approx(item.tokens)}</span>
      </div>
    )
  }
  const groupRows = (c: Category, g: Group) => {
    const limit = shown[g.id] ?? FIRST
    const rest = g.items.length - limit
    return (
      <div key={g.id} role="group" aria-label={g.name}>
        {c.groups.length > 1 && (
          <div className={cn(ROW, 'cursor-pointer grid-cols-[20px_minmax(0,1fr)_auto_auto_32px] pl-8 sm:pl-12')} onClick={() => onChange(toggle(g.keys, selected))}>
            <Check state={tri(g.keys, selected)} label={`Select all ${g.name}`} onChange={() => onChange(toggle(g.keys, selected))} />
            <span className="truncate font-medium">{g.name}</span>
            <Count keys={g.keys} selected={selected} />
            <Tokens value={g.tokens} max={c.tokens} color={c.color} />
            <span />
          </div>
        )}
        {g.items.slice(0, limit).map(itemRow)}
        {rest > 0 && (
          <div className="py-1 pl-[68px] sm:pl-[108px]">
            <button type="button" data-nav="" className="min-h-11 text-xs text-fg-accent hover:underline" onClick={() => setShown({ ...shown, [g.id]: limit + PAGE })}>
              Show {Math.min(rest, PAGE)} more
            </button>
          </div>
        )}
      </div>
    )
  }
  const section = (id: Category['section'], title: string, aside?: string) => {
    const list = cats.filter((c) => c.section === id)
    if (list.length === 0) return null
    return (
      <section aria-label={title}>
        <h3 className="flex justify-between px-4 pt-3 pb-1 text-[11px] font-normal tracking-wider text-fg-tertiary uppercase">
          <span>{title}</span>
          {aside && <span>{aside}</span>}
        </h3>
        {list.map((c) => (
          <div key={c.kind}>
            <div className={cn(ROW, 'cursor-pointer grid-cols-[20px_10px_minmax(0,1fr)_auto_auto_32px] pl-4')} onClick={() => flip(c.kind)}>
              {c.section === 'output' ? <Check state={tri(c.keys, selected)} label={`Select all ${c.name}`} onChange={() => onChange(toggle(c.keys, selected))} /> : <span />}
              <span className="size-2.5 rounded-[3px]" style={{ background: c.color }} />
              <span className="min-w-0">
                <span className="block truncate font-medium">{c.name}</span>
                {c.groups.length > 1 && <span className="block truncate text-xs text-fg-tertiary">{c.groups.map((g) => `${g.items.length} ${g.name.toLowerCase()}`).join(' · ')}</span>}
              </span>
              <Count keys={c.keys} selected={selected} />
              <Tokens value={c.tokens} max={max} color={c.color} />
              <Button
                variant="ghost"
                size="icon-sm"
                data-nav=""
                className="text-fg-secondary"
                aria-expanded={open.has(c.kind)}
                aria-label={`Show ${c.name}`}
                onClick={(e) => {
                  e.stopPropagation()
                  flip(c.kind)
                }}
              >
                <ChevronRight className={cn('transition-transform', open.has(c.kind) && 'rotate-90')} />
              </Button>
            </div>
            {open.has(c.kind) && c.groups.map((g) => groupRows(c, g))}
          </div>
        ))}
      </section>
    )
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto pb-2 text-sm" onKeyDown={nav}>
      {section('output', 'Attachments & output')}
      {section('conversation', 'Conversation', 'pick items · no select-all')}
    </div>
  )
}

function Count({ keys, selected }: { keys: string[]; selected: Set<string> }) {
  const n = keys.reduce((s, k) => s + (selected.has(k) ? 1 : 0), 0)
  return <span className="w-10 text-right text-xs tabular-nums text-fg-secondary">{n ? `${n}/${keys.length}` : keys.length}</span>
}

function Note({ title, children }: { title: string; children: ReactNode }) {
  return (
    <p className="mx-4 mt-2.5 rounded-lg border border-warning-border px-3 py-2.5 text-xs">
      <b className="font-semibold">{title}</b> {children}
    </p>
  )
}

/** 197 §8 review: grouped lines with tokens, the companions, the three warnings, and 48's retention line. `items` is the whole inventory. */
export function ReviewBody({ cats, items, selected }: { cats: Category[]; items: ContextItem[]; selected: Set<string> }) {
  const r = useMemo(() => review(cats, items, selected), [cats, items, selected])
  return (
    <div className="min-h-0 flex-1 overflow-auto pb-3 text-sm">
      <ul className="px-4 pt-1.5" aria-label="Will be removed">
        {r.lines.map((l) => (
          <li key={l.label} className="flex justify-between gap-2.5 border-b border-dashed py-2 last:border-b-0">
            <span>{l.label}</span>
            <span className="text-fg-secondary tabular-nums">{approx(l.tokens)}</span>
          </li>
        ))}
        {r.companions.count > 0 && (
          <li className="flex justify-between gap-2.5 py-2 text-fg-secondary">
            <span>
              + {r.companions.count} protocol companion{r.companions.count === 1 ? '' : 's'} (the tool call/result group of a selected message)
            </span>
            <span className="tabular-nums">{approx(r.companions.tokens)}</span>
          </li>
        )}
      </ul>
      {r.summary && <Note title="Summary selected.">Removing it loses part of the earlier conversation.</Note>}
      {r.diagrams && <Note title="Diagrams">are cut out of their replies; the reply text stays, with a one-line marker where the diagram was.</Note>}
      {r.cards && <Note title="Pages, apps and shown files">keep their card in your transcript; the model sees a one-line marker instead of their source.</Note>}
      <p className="mx-4 mt-2.5 rounded-lg border px-3 py-2.5 text-xs text-fg-secondary">
        Your transcript keeps everything. Removed items collapse to <b>Removed from context</b> with <b>View original</b>. There is no undo:
        send or attach something again to bring it back.
      </p>
    </div>
  )
}
