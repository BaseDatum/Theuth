import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { useHarnesses } from '@/link/harnesses'
import type { Api } from '@/rpc/api'
import type { ContextItem } from '@/rpc/bindings/ContextItem'
import type { ContextKind } from '@/rpc/bindings/ContextKind'
import type { ContextSnapshot } from '@/rpc/bindings/ContextSnapshot'
import type { ContextTarget } from '@/rpc/bindings/ContextTarget'
import { RpcError } from '@/rpc/client'
import { scope } from '@/store/harness-scope'

/** `/shake <category>` (48 §2.1): literal categories, not a selector language. */
export type ShakeCategory = 'images' | 'documents' | 'messages' | 'tools' | 'summary'

const KIND: Record<ShakeCategory, ContextKind> = {
  images: 'image',
  documents: 'document',
  messages: 'message',
  tools: 'tool',
  summary: 'summary',
}

type Request = {
  session: string
  category?: ShakeCategory
  targets?: ContextTarget[]
  /** Revision of the history the button was rendered from; a differing inventory refuses rather than retargets. */
  expectedRevision?: string
}

/**
 * Open the shared context picker (48 §0.1): the composer's slash command, the
 * session menu and the transcript's remove buttons all end here. `targets`
 * preselects exact server-issued targets (a button, with the displayed
 * `expectedRevision`); `category` filters and, except for `messages`,
 * preselects; bare opens the whole inventory unselected.
 */
export function openContextPicker(
  session: string,
  category?: ShakeCategory,
  targets?: ContextTarget[],
  expectedRevision?: string,
): void {
  window.dispatchEvent(
    new CustomEvent<Request>('wth:shake', { detail: { session, category, targets, expectedRevision } }),
  )
}

/** Stable identity of a target; `part`/`image` address original stored arrays (48 §3.1). */
function key(t: ContextTarget): string {
  switch (t.kind) {
    case 'message':
      return `m${t.seq}`
    case 'head':
      return `h${t.epoch}`
    case 'binary':
      return `${key(t.owner)}/${t.part}${t.image === null ? '' : `.${t.image}`}`
  }
}

/**
 * The exact set `remove_context` receives: selected targets plus their
 * `remove_with` closure (iterated until stable), deduped; a whole message or head
 * subsumes its own binaries (48 §3.2).
 */
export function expand(
  items: ContextItem[],
  selected: Set<string>,
): { targets: ContextTarget[]; companions: ContextItem[] } {
  const byKey = new Map(items.map((i) => [key(i.target), i]))
  const set = new Map<string, ContextTarget>()
  const companions: ContextItem[] = []
  const queue = [...selected]
  for (let k = queue.shift(); k !== undefined; k = queue.shift()) {
    const item = byKey.get(k)
    if (!item || set.has(k)) continue
    set.set(k, item.target)
    if (!selected.has(k)) companions.push(item)
    for (const t of item.remove_with) queue.push(key(t))
  }
  const targets = [...set.values()].filter(
    (t) => t.kind !== 'binary' || !set.has(key(t.owner)),
  )
  return { targets, companions }
}

const EMPTY: Record<ShakeCategory, string> = {
  images: 'No images in current context',
  documents: 'No documents in current context',
  messages: 'No messages in current context',
  tools: 'No tool exchanges in current context',
  summary: 'No summary in current context',
}

const ROLE: Record<ContextKind, string> = {
  message: 'Message',
  image: 'Image',
  document: 'Document',
  tool: 'Tool',
  summary: 'Summary',
}

type Phase =
  | { at: 'loading' }
  | { at: 'pick'; snap: ContextSnapshot; items: ContextItem[]; selected: Set<string> }
  | { at: 'review'; snap: ContextSnapshot; items: ContextItem[]; selected: Set<string> }
  | { at: 'pending'; snap: ContextSnapshot; items: ContextItem[]; selected: Set<string> }
  | { at: 'error'; message: string; retry: boolean }

/**
 * Mounted once in `Composer`. Fetches the server inventory of the whole active
 * context, lets the user pick, reviews the exact expanded set, then sends one
 * `remove_context`. No optimistic transcript surgery: success emits
 * `wth:reload-session` and a toast; failure keeps everything and never retries
 * a destructive RPC by itself.
 */
export function ContextPicker({ session }: { session: string }) {
  const [req, setReq] = useState<(Request & { api: Api; harness: string }) | null>(null)
  const [phase, setPhase] = useState<Phase>({ at: 'loading' })

  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<Request>).detail
      if (d.session !== session) return
      // Capture the initiating harness: a reply after a harness switch must not touch the other harness (48 §5).
      const harness = useHarnesses.getState().active
      const api = scope(harness).connection.getState().api
      if (!api) return toast.error('Not connected')
      setReq({ ...d, api, harness })
    }
    window.addEventListener('wth:shake', on)
    return () => window.removeEventListener('wth:shake', on)
  }, [session])

  // Inventory load; `req` identity guards a reply for a picker already closed or reopened.
  useEffect(() => {
    if (!req) return
    let live = true
    setPhase({ at: 'loading' })
    req.api.context_items(req.session).then(
      (snap) => {
        if (!live) return
        if (!snap.editable) {
          setPhase({ at: 'error', message: snap.reason ?? 'Session is busy; wait or stop it, then retry.', retry: true })
          return
        }
        // A button rendered from an older history must not retarget a reused seq: refuse, the reload refreshes it.
        if (req.expectedRevision !== undefined && req.expectedRevision !== snap.revision) {
          setPhase({ at: 'error', message: 'The context changed since this was shown. Reload the transcript and review again.', retry: false })
          return
        }
        // The list filters what is selectable; closure and review always run over the whole inventory.
        const kind = req.category && KIND[req.category]
        const items = kind ? snap.items.filter((i) => i.kind === kind) : snap.items
        const byKey = new Set(items.map((i) => key(i.target)))
        const selected = new Set<string>()
        if (req.targets) {
          for (const t of req.targets) if (byKey.has(key(t))) selected.add(key(t))
          // A button whose target left the context (already removed): nothing to preselect, so refuse rather than guess.
          if (selected.size === 0) {
            setPhase({ at: 'error', message: 'That item is no longer in the current context.', retry: false })
            return
          }
        } else if (req.category && req.category !== 'messages') {
          for (const k of byKey) selected.add(k)
        }
        setPhase({ at: selected.size > 0 ? 'review' : 'pick', snap, items, selected })
      },
      (e: unknown) => live && setPhase({ at: 'error', message: e instanceof Error ? e.message : String(e), retry: true }),
    )
    return () => {
      live = false
    }
  }, [req])

  const close = () => {
    if (phase.at === 'pending') return
    setReq(null)
  }

  const confirm = async () => {
    if (phase.at !== 'review' || !req) return
    const { snap, items, selected } = phase
    const { targets } = expand(snap.items, selected)
    // Local gate before the destructive call: the server rechecks, but a session that went busy, a slice marked stale
    // or a newer revision while the dialog sat open means the reviewed set no longer describes the context.
    const st = scope(req.harness)
    const slice = st.sessions.getState().sessions[req.session]
    if (st.connection.getState().state !== 'open') {
      setPhase({ at: 'error', message: 'Not connected. Review again once reconnected.', retry: true })
      return
    }
    if (slice?.running || slice?.contextStale) {
      setPhase({ at: 'error', message: 'Session is busy; wait or stop it, then retry.', retry: true })
      return
    }
    if (slice?.contextRevision !== undefined && slice.contextRevision !== snap.revision) {
      setPhase({ at: 'error', message: 'The context changed while you were reviewing. Review again.', retry: true })
      return
    }
    setPhase({ at: 'pending', snap, items, selected })
    try {
      const r = await req.api.remove_context(req.session, snap.revision, targets)
      const parts = [
        r.messages > 0 && `${r.messages} message${r.messages === 1 ? '' : 's'}`,
        r.images > 0 && `${r.images} image${r.images === 1 ? '' : 's'}`,
        r.documents > 0 && `${r.documents} document${r.documents === 1 ? '' : 's'}`,
        r.heads > 0 && `${r.heads} summar${r.heads === 1 ? 'y' : 'ies'}`,
      ].filter((p): p is string => typeof p === 'string')
      toast(parts.length ? `Removed from context: ${parts.join(', ')} · ~${r.estimated_tokens} tokens now` : 'Nothing changed')
      // `ContextChanged` marks the slice stale; the settled reload here only targets the harness still on screen.
      if (useHarnesses.getState().active === req.harness)
        window.dispatchEvent(
          new CustomEvent('wth:reload-session', { detail: { session: req.session, settled: true } }),
        )
      setReq(null)
    } catch (e) {
      // A server refusal (stale, busy, malformed) wrote nothing; a transport failure has an unknown outcome. Either way:
      // show it, reload the inventory on request, never replay the RPC.
      const m = e instanceof Error ? e.message : String(e)
      const unknown = (e instanceof RpcError && e.code === -32300) || st.connection.getState().state !== 'open'
      setPhase({
        at: 'error',
        message: unknown
          ? `Connection lost; whether the removal committed is unknown. Review again before retrying. (${m})`
          : `Nothing was changed: ${m}`,
        retry: true,
      })
    }
  }

  const reload = () => req && setReq({ ...req })

  const title = req?.category ? `Shake context: ${req.category}` : 'Shake context'

  return (
    <Dialog open={req !== null} onOpenChange={(o) => !o && close()}>
      <DialogContent
        className="flex max-h-[85vh] flex-col sm:max-w-lg"
        showCloseButton={phase.at !== 'pending'}
        onCloseAutoFocus={(e) => {
          // The opener may be a transcript button unmounted by the reload; the composer is always there.
          e.preventDefault()
          window.dispatchEvent(new Event('wth:focus-composer'))
        }}
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {phase.at === 'loading' && 'Reading the active context…'}
            {(phase.at === 'pick' || phase.at === 'review' || phase.at === 'pending') &&
              `Whole active context · ~${phase.snap.estimated_tokens} tokens · epoch ${phase.snap.epoch}`}
            {phase.at === 'error' && 'Remove from context'}
          </DialogDescription>
        </DialogHeader>
        {phase.at === 'error' && <p className="text-sm">{phase.message}</p>}
        {phase.at === 'pick' &&
          (phase.items.length === 0 ? (
            <p className="text-sm">{req?.category ? EMPTY[req.category] : 'Nothing in current context'}</p>
          ) : (
            <Pick
              items={phase.items}
              selected={phase.selected}
              onChange={(selected) => setPhase({ ...phase, selected })}
            />
          ))}
        {(phase.at === 'review' || phase.at === 'pending') && <Review items={phase.snap.items} selected={phase.selected} />}
        <DialogFooter>
          {phase.at === 'error' && phase.retry && (
            <Button variant="outline" onClick={reload}>
              Review again
            </Button>
          )}
          {phase.at === 'review' && !req?.targets && (
            <Button variant="outline" onClick={() => setPhase({ ...phase, at: 'pick' })}>
              Edit selection
            </Button>
          )}
          <Button variant="outline" disabled={phase.at === 'pending'} onClick={close}>
            Cancel
          </Button>
          {phase.at === 'pick' && phase.items.length > 0 && (
            <Button disabled={phase.selected.size === 0} onClick={() => setPhase({ ...phase, at: 'review' })}>
              Review{phase.selected.size > 0 ? ` (${phase.selected.size})` : ''}
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

/** Checkbox rows, 44 px tall for fingers (48 §2.3); Space toggles, Enter is left to the footer's default button. */
function Pick({
  items,
  selected,
  onChange,
}: {
  items: ContextItem[]
  selected: Set<string>
  onChange(s: Set<string>): void
}) {
  const toggle = (k: string) => {
    const next = new Set(selected)
    if (!next.delete(k)) next.add(k)
    onChange(next)
  }
  return (
    <ul className="-mx-2 min-h-0 flex-1 overflow-auto text-sm" aria-label="Items in the active context">
      {items.map((i) => {
        const k = key(i.target)
        return (
          <li key={k}>
            <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-lg px-2 py-2 hover:bg-layer-2 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring">
              <input
                type="checkbox"
                className="size-4 shrink-0 accent-foreground"
                checked={selected.has(k)}
                onChange={() => toggle(k)}
              />
              <Row item={i} />
            </label>
          </li>
        )
      })}
    </ul>
  )
}

function Row({ item }: { item: ContextItem }) {
  return (
    <span className="min-w-0 flex-1">
      <span className="text-xs text-fg-tertiary">
        {ROLE[item.kind]}
        {(item.target.kind === 'head' || (item.target.kind === 'binary' && item.target.owner.kind === 'head')) &&
          ' · summary head'}
        {item.remove_with.length > 0 && ` · removes ${item.remove_with.length} more`}
      </span>
      <span className="block truncate">{item.label}</span>
    </span>
  )
}

/** The exact set to be removed, its protocol companions, and every limit the user is agreeing to (48 §2.1–2.3). `items` is the whole inventory. */
function Review({ items, selected }: { items: ContextItem[]; selected: Set<string> }) {
  const { targets, companions } = expand(items, selected)
  const keys = new Set(targets.map(key))
  const chosen = items.filter((i) => selected.has(key(i.target)) && keys.has(key(i.target)))
  const subsumed = items.filter((i) => selected.has(key(i.target)) && !keys.has(key(i.target)))
  const messages = targets.filter((t) => t.kind === 'message').length
  const attachments = targets.filter((t) => t.kind === 'binary').length
  const heads = targets.filter((t) => t.kind === 'head').length
  const snapImage = targets.some((t) => t.kind === 'binary' && t.owner.kind === 'head')
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 text-sm">
      <p className="text-xs text-fg-tertiary">
        {messages} message{messages === 1 ? '' : 's'} · {attachments} attachment{attachments === 1 ? '' : 's'} ·{' '}
        {heads} summar{heads === 1 ? 'y' : 'ies'} · {companions.length} protocol companion
        {companions.length === 1 ? '' : 's'}
      </p>
      <ul className="min-h-0 flex-1 overflow-auto" aria-label="Will be removed">
        {chosen.map((i) => (
          <li key={key(i.target)} className="py-1">
            <Row item={i} />
          </li>
        ))}
        {companions.map((i) => (
          <li key={key(i.target)} className="py-1 pl-4 border-l-2 border-border-subtle">
            <Row item={i} />
            <span className="block text-xs text-fg-tertiary">Required with the selected message</span>
          </li>
        ))}
        {subsumed.map((i) => (
          <li key={key(i.target)} className="py-1 text-fg-tertiary">
            <Row item={i} />
            <span className="block text-xs">Included in its whole message</span>
          </li>
        ))}
      </ul>
      <ul className="list-disc space-y-1 pl-4 text-xs text-fg-secondary" aria-label="Limits">
        {snapImage && <li>Summary image: removing this loses part of the earlier conversation.</li>}
        {heads > 0 && <li>Removing the summary loses the earlier context it stood for; the tail boundary stays.</li>}
        {companions.length > 0 && <li>Whole messages take their tool call/result group with them.</li>}
        <li>Original history stays in the archive as “Removed from context”; later replies stay.</li>
        <li>No undo: /undo and /redo do not restore this. Attach or send again to reintroduce it.</li>
      </ul>
    </div>
  )
}
