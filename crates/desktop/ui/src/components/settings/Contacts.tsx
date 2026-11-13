import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Bot, CircleCheck, Copy, MessageSquare, MoreHorizontal, Plus, ShieldAlert, X } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { Fragment, type ReactNode, type Ref, useCallback, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { type PairView, pairView, spaced } from '@/components/settings/PairDialog'
import { Empty, RenameInput, Section, errorText, useAction } from '@/components/settings/shared'
import type { TabProps } from '@/components/settings/types'
import { useAgents, useMesh } from '@/components/tasks/queries'
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
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { chatName } from '@/lib/attach'
import { PAIRED_RULE, SHARE_RULE, type Sharing, accessFocus, exportName, fingerprint, focusedContact, names, rogueLine, useContacts, useExports, useFirstWaiting, useSharing } from '@/lib/contacts'
import { engineLabel } from '@/lib/owner'
import { putPair, queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/bindings/client'
import type { Contact } from "@/rpc/bindings/method.a2a.output"
import type { ContactMachine } from "@/rpc/bindings/method.a2a.output"
import type { ContactPatch } from '@/rpc/bindings/method.a2a.input'
import type { ContactTarget } from '@/rpc/bindings/method.a2a.input'
import type { ContactWindow } from "@/rpc/bindings/method.a2a.output"
import type { Export } from "@/rpc/bindings/method.a2a.output"
import type { ExportTarget } from "@/rpc/bindings/method.a2a.output"
import type { FirstContact } from "@/rpc/bindings/method.a2a.output"
import type { FirstWaiting } from "@/rpc/bindings/method.a2a.output"
import type { PairCeremony } from "@/rpc/bindings/method.a2a.output"
import type { PeerPath } from "@/rpc/bindings/method.a2a.output"

const NO_PATCH: ContactPatch = { name: null, blocked: null }
const PATH: Record<PeerPath, [string, string]> = { lan: ['LAN', 'bg-success'], relay: ['Relay', 'bg-warning'], offline: ['offline', 'ring-1 ring-fg-tertiary ring-inset'] }
const CHIP = 'inline-flex items-center gap-1 rounded-full border border-border-subtle px-2 py-0.5 text-xs text-fg-secondary'
const FIRST: [FirstContact, string][] = [
  ['ask', 'Ask first'],
  ['allow', 'Always allow'],
  ['block', 'Block'],
]
/** 188: Settings › Network › Contacts, scrolled to `contact`'s Access row when one is named. */
const toContacts = (contact?: string) => ({ to: '/settings/$tab', params: { tab: 'contacts' }, search: contact ? { focus: accessFocus(contact) } : {} }) as const
/** 191 D8: the alert's way out may be to unpair one of ours. */
const toMachines = { to: '/settings/$tab', params: { tab: 'machines' } } as const
/** 191 D8: a quarantined record (refused or waiting) is not admitted here: no menu; Access and Share… list it with its state and its mesh-wide shares. Contact RPCs name it by record id, since a waiting rename shares its alias with a live contact. */
const held = (c: Contact) => c.problem_kind === 'refused' || c.problem_kind === 'waiting'
const who = (c: Contact) => (held(c) && c.id ? c.id : c.name)
/** 191: why a held contact reaches nothing from this machine now, and where its shares apply. */
const heldLine = (c: Contact) =>
  c.problem_kind === 'waiting' ? `Waiting on this machine: ${c.problem}. What you share applies once this clears.` : `Refused on this machine: ${c.problem}. What you share applies on your machines that admit ${c.name}.`

/** 191 D8: `Their machines: laptop · studio`, this machine's accepted roster of the contact; one its history lists but its roster no longer does is struck through, labelled left. */
export function TheirMachines({ machines }: { machines: readonly ContactMachine[] }) {
  if (machines.length === 0) return null
  return (
    <p className="break-words text-[13px] text-fg-secondary">
      Their machines:{' '}
      {machines.map((m, i) => (
        <Fragment key={m.eid}>
          {i > 0 && ' · '}
          {m.left ? (
            <span className="text-muted-foreground">
              <s title={m.eid}>{m.name}</s> left
            </span>
          ) : (
            <span title={m.eid}>{m.name}</span>
          )}
        </Fragment>
      ))}
    </p>
  )
}

/** 191 D8: a row's problem and what `problem_kind` offers: refused → Remove; waiting → nothing (it clears by itself); alert → Remove and Network › Machines. */
function Problem({ c, onRemove, onMachines }: { c: Contact; onRemove: () => void; onMachines: () => void }) {
  if (!c.problem || !c.problem_kind) return null
  const kind = c.problem_kind
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1.5">
      <p role="status" className={cn('min-w-0 flex-1 basis-56 break-words text-[13px]', kind === 'waiting' ? 'text-muted-foreground' : 'text-destructive')}>
        {c.problem}
        {kind === 'waiting' && ' · this clears by itself'}
      </p>
      {kind !== 'waiting' && (
        <span className="flex shrink-0 items-center gap-2">
          {kind === 'alert' && (
            <Button variant="ghost" size="sm" className="pointer-coarse:min-h-11" onClick={onMachines}>
              Network › Machines
            </Button>
          )}
          <Button variant="outline" size="sm" className="pointer-coarse:min-h-11" onClick={onRemove}>
            Remove…
          </Button>
        </span>
      )}
    </div>
  )
}

function PathChip({ path }: { path: PeerPath }) {
  const [text, dot] = PATH[path]
  return (
    <span className={CHIP}>
      <span className={cn('size-1.5 rounded-full', dot)} aria-hidden="true" />
      {text}
    </span>
  )
}

/** 183 review 1: how many first messages wait for one (contact, export), when more than one. */
export const waitingCount = (w: FirstWaiting) => (w.waiting > 1 ? ` · ${w.waiting} messages` : '')

/** 183 Phase 3: the line under a held first message — shared by Network › Messaging, Contacts and the strip. */
export const waitingLine = (w: FirstWaiting) =>
  `${w.contact}${w.machine ? ` (${w.machine})` : ''} is waiting for you to allow its first message to ${w.export}${waitingCount(w)}`

const ANSWERED: Record<FirstContact, (c: string) => string> = { ask: (c) => `Allowed ${c} once`, allow: (c) => `Allowed ${c} · always`, block: (c) => `Blocked ${c}` }

/** Answer a held first message (`contact_first_answer`); the engine fires `A2aChanged`, which drops it from every list. */
export function useFirstAnswer() {
  const api = useApi()
  return useAction((v: { contact: string; exp: string; answer: FirstContact }) => api.contact_first_answer(v.contact, v.exp, v.answer), {
    keys: [queryKeys.a2a],
    onSuccess: (_, v) => toast(ANSWERED[v.answer](v.contact)),
  })
}

/**
 * Allow once · Always · Block for `contact`'s held first message to `exp`; one answer per row (a second click would only
 * set the standing default). 191: a message another machine holds (`elsewhere`) offers no Allow once (that is the
 * holder's); Always and Block reach it from any machine.
 */
export function FirstButtons({ contact, exp, elsewhere = false }: { contact: string; exp: string; elsewhere?: boolean }) {
  const answer = useFirstAnswer()
  return (
    <span className="flex shrink-0 items-center gap-2">
      {(
        [
          ['ask', 'Allow once'],
          ['allow', 'Always'],
          ['block', 'Block'],
        ] as const
      )
        .filter(([v]) => !(elsewhere && v === 'ask'))
        .map(([v, label]) => (
          <Button
            key={v}
            variant={v === 'block' ? 'ghost' : 'outline'}
            size="sm"
            className="pointer-coarse:min-h-11"
            disabled={answer.isPending || answer.isSuccess}
            onClick={() => answer.mutate({ contact, exp, answer: v })}
          >
            {label}
          </Button>
        ))}
    </span>
  )
}

/** One held first message: its line and its answers. */
export function WaitingRow({ w }: { w: FirstWaiting }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-lg bg-layer-2 px-2.5 py-1.5 text-[13px]">
      <span className="min-w-0 flex-1 basis-56 break-words">{waitingLine(w)}</span>
      <FirstButtons contact={w.contact} exp={w.export} elsewhere={w.elsewhere} />
    </div>
  )
}

/** One export as a chip: an agent or a chat (191: `on xeus` for a chat on another machine), and why it is paused (W-41, §12.10). */
export function ExportChip({ name, exp, on }: { name: string; exp: Export | undefined; on?: string | null }) {
  const Icon = exp?.target.kind === 'agent' ? Bot : MessageSquare
  return (
    <span className={cn(CHIP, 'max-w-full', exp?.paused && 'text-fg-tertiary')}>
      <Icon className="size-3 shrink-0" aria-hidden="true" />
      <span className="truncate">
        {exp?.description || name}
        {on && ` · on ${on}`}
        {exp?.paused && ` · Paused (${exp.paused})`}
      </span>
    </span>
  )
}

/** 191: the machine a shared chat lives on when it is not this one (`null` here, for an agent, or unknown), from the sessions list. */
function useChatMachine(): (exp: Export | undefined) => string | null {
  const canQuery = useQuerySupport(['list_sessions'])
  const api = useApi()
  const mesh = useMesh()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canQuery })
  return (exp) => {
    const t = exp?.target
    if (t?.kind !== 'chat') return null
    const s = sessions.data?.find((x) => x.id === t.session)
    return s && !s.here ? engineLabel(s.owner, mesh.data?.peers) : null
  }
}

/** 188 Settings › Network › Contacts: the contacts, then what each may reach; `?focus=access:<name>` scrolls to one. */
export function ContactsTab({ focus }: TabProps) {
  return (
    <>
      <Contacts />
      <ContactAccess focus={focusedContact(focus)} />
    </>
  )
}

/**
 * 183 §12.7: the contacts list — each contact with its machine (from its card), path, fingerprint and the exports it
 * may reach; ⋯ Rename · Choose what it may reach · Block · Remove. Add contact… opens the window. 191 D8: every machine
 * of the mesh lists the same contacts; their machines and the problem are this machine's own view.
 */
export function Contacts({ adding: initial = false }: { adding?: boolean }) {
  const api = useApi()
  const navigate = useNavigate()
  const contacts = useContacts()
  const exports = useExports()
  const waiting = useFirstWaiting().data ?? []
  const machineOf = useChatMachine()
  const [adding, setAdding] = useState(initial)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [confirm, setConfirm] = useState<{ c: Contact; block: boolean } | null>(null)
  const keys = [queryKeys.contacts]
  const update = useAction((v: { name: string; patch: Partial<ContactPatch> }) => api.contact_update(v.name, { ...NO_PATCH, ...v.patch }), { keys })
  const remove = useAction((v: { name: string; block: boolean }) => api.contact_remove(v.name, v.block), { keys })
  return (
    <Section
      title="Contacts"
      actions={
        <Button variant="outline" size="sm" onClick={() => setAdding(true)}>
          <Plus /> Add contact…
        </Button>
      }
    >
      <p className="text-[13px] text-fg-secondary">Other people's machines. {SHARE_RULE}</p>
      {contacts.isPending && <Empty>Loading…</Empty>}
      {contacts.isError && <Empty>{errorText(contacts.error)}</Empty>}
      {contacts.data?.length === 0 && <Empty>No contacts yet.</Empty>}
      {contacts.data?.map((c) => (
        <div key={c.id || c.name} className={cn('flex flex-wrap items-start gap-x-3 gap-y-2 border-b border-border-subtle py-3 last:border-0', c.blocked && 'text-fg-secondary')}>
          <div className="min-w-0 flex-1 basis-56">
            <div className="flex flex-wrap items-center gap-1.5 text-sm font-medium">
              {renaming === c.name ? <RenameInput value={c.name} label="Contact name" onSave={(name) => update.mutate({ name: c.name, patch: { name } })} onDone={() => setRenaming(null)} /> : c.name}
              {c.blocked && <span className="rounded-full bg-danger/10 px-1.5 text-xs text-danger">blocked</span>}
            </div>
            <div className="text-[13px] text-fg-secondary">
              {c.machine}
              <span className="ml-2 font-mono text-xs text-fg-tertiary" title={c.endpoint}>
                {c.fingerprint || fingerprint(c.endpoint)}
              </span>
            </div>
            <TheirMachines machines={c.machines} />
            {c.paired_on && <p className="text-[13px] text-fg-secondary">Paired on {c.paired_on}</p>}
            <Problem c={c} onRemove={() => setConfirm({ c, block: false })} onMachines={() => void navigate(toMachines)} />
            {(!held(c) || c.exports.length > 0) && (
              <div className="mt-1.5 flex flex-wrap gap-1">
                {c.exports.length === 0 ? (
                  <span className="text-[13px] text-fg-tertiary">Nothing shared yet</span>
                ) : (
                  c.exports.map((n) => {
                    const x = exports.data?.find((e) => e.name === n)
                    return <ExportChip key={n} name={n} exp={x} on={machineOf(x)} />
                  })
                )}
              </div>
            )}
            {c.shared.length > 0 && (
              <p className="mt-1 break-words text-[13px] text-fg-secondary">
                Shared with you: {c.shared.map((o) => `${o.title}${o.state === 'ask_first' ? ' (asks first)' : o.state === 'unavailable' ? ' (paused)' : ''}`).join(', ')}
              </p>
            )}
            {waiting
              .filter((w) => w.contact === c.name)
              .map((w) => (
                <div key={w.export} className="mt-1.5">
                  <WaitingRow w={w} />
                </div>
              ))}
          </div>
          <div className="ml-auto flex items-center gap-2">
            <PathChip path={c.path} />
            {!held(c) && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon-sm" aria-label={`${c.name}: Rename · Block · Remove`}>
                    <MoreHorizontal />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => setRenaming(c.name)}>Rename</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => void navigate(toContacts(c.name))}>Choose what {c.name} may reach…</DropdownMenuItem>
                  {c.blocked ? (
                    <DropdownMenuItem onSelect={() => update.mutate({ name: c.name, patch: { blocked: false } })}>Unblock</DropdownMenuItem>
                  ) : (
                    <DropdownMenuItem onSelect={() => setConfirm({ c, block: true })}>Block…</DropdownMenuItem>
                  )}
                  <DropdownMenuItem variant="destructive" onSelect={() => setConfirm({ c, block: false })}>
                    Remove…
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>
        </div>
      ))}
      {adding && <AddContact onClose={() => setAdding(false)} onAccess={(peer) => void navigate(toContacts(contacts.data?.find((c) => c.endpoint === peer)?.name))} />}
      <AlertDialog open={confirm !== null} onOpenChange={(o) => !o && setConfirm(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirm?.block ? 'Block' : 'Remove'} {confirm?.c.name}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirm?.block
                ? `${confirm.c.name} is refused on all your machines from now on and its open tasks close. Unblock it later from this list.`
                : `${confirm?.c.name} is forgotten on all your machines and its open tasks close. Adding it again needs a new code check.`}
            </AlertDialogDescription>
            {confirm && !confirm.block && <p className="text-sm text-fg-secondary">{rogueLine(confirm.c.name)}</p>}
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (confirm) remove.mutate({ name: who(confirm.c), block: confirm.block })
                setConfirm(null)
              }}
            >
              {confirm?.block ? 'Block' : 'Remove'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Section>
  )
}

/** What the ceremony shows: the pair dialog's views, plus `mismatch` when this side pressed "The codes differ". */
export type CeremonyView = PairView | 'mismatch'

const LIMITED_ID = 'contact-limited-trust'

function Limited({ id, children }: { id?: string; children: ReactNode }) {
  return (
    <div className="flex items-start gap-2 rounded-lg border border-border-subtle bg-layer-2 px-3 py-2 text-[13px] text-fg-secondary">
      <ShieldAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <p id={id}>{children}</p>
    </div>
  )
}

/** 183 §7.4, §12.7: one contact ceremony — the code, who it is (machine and fingerprint), Accept / Cancel / The codes differ, and how it ended. */
export function CeremonyBody({
  name,
  detail,
  ceremony,
  view,
  error,
  onAccept,
  onCancel,
  onMismatch,
  onBack,
  onDone,
  onAccess,
  cancelRef,
}: {
  name: string
  detail: string
  ceremony: PairCeremony | undefined
  view: CeremonyView
  error: string | null
  onAccept: () => void
  onCancel: () => void
  onMismatch: () => void
  onBack: () => void
  onDone: () => void
  onAccess: () => void
  /** Cancel (or Done / Back) takes the initial focus, never Accept. */
  cancelRef?: Ref<HTMLButtonElement>
}) {
  const back = (
    <Button ref={cancelRef} variant="outline" onClick={onBack}>
      Back
    </Button>
  )
  const end: Partial<Record<CeremonyView, [ReactNode, ReactNode]>> = {
    starting: [
      <p className="text-sm text-fg-secondary">Connecting to {name}…</p>,
      <Button ref={cancelRef} variant="outline" onClick={onCancel}>
        Cancel
      </Button>,
    ],
    failed: [<p className="text-sm text-danger">{error}</p>, back],
    paired: [
      <p className="flex items-start gap-2 text-sm">
        <CircleCheck className="mt-0.5 size-4 shrink-0 text-success" aria-hidden="true" />
        {name} is now a contact. It reaches nothing until you share an agent or a chat with it.
      </p>,
      <>
        <Button variant="outline" onClick={onAccess}>
          Choose what {name} may reach
        </Button>
        <Button ref={cancelRef} onClick={onDone}>
          Done
        </Button>
      </>,
    ],
    rejected: [<p className="text-sm text-danger">Not added: {name} pressed Cancel, or saw a different code.</p>, back],
    mismatch: [<p className="text-sm text-danger">Not added: the codes did not match. Someone else may be answering as {name}; nothing was stored.</p>, back],
    expired: [<p className="text-sm text-fg-secondary">The check timed out before both sides pressed Accept. Nothing was stored.</p>, back],
  }
  const ended = end[view]
  return (
    <div className="flex flex-col gap-5">
      <div>
        <DialogTitle className="text-lg font-semibold">Add {name} as a contact</DialogTitle>
        {detail && <p className="font-mono text-[13px] text-fg-tertiary">{detail}</p>}
      </div>
      {ended ? (
        <>
          {ended[0]}
          <div className="flex flex-wrap justify-end gap-2">{ended[1]}</div>
        </>
      ) : (
        <>
          <div className="rounded-xl bg-layer-2 px-4 py-6 text-center">
            <div className="font-mono text-5xl font-semibold tracking-[0.15em] tabular-nums" role="group" aria-label="Contact code">
              {spaced(ceremony?.code ?? '')}
            </div>
            <p className="mt-3 text-sm text-fg-secondary">Check that {name} shows the same code</p>
          </div>
          <Limited id={LIMITED_ID}>{PAIRED_RULE}</Limited>
          <div className="flex flex-wrap items-center justify-end gap-2">
            {view === 'waiting' ? (
              <span className="mr-auto text-[13px] text-fg-secondary">Waiting for {name} to press Accept…</span>
            ) : (
              <Button variant="link" className="mr-auto px-0 text-[13px] text-fg-secondary" onClick={onMismatch}>
                The codes differ
              </Button>
            )}
            <Button onClick={onAccept} disabled={view === 'waiting'}>
              Accept
            </Button>
            <Button ref={cancelRef} variant="outline" onClick={onCancel}>
              Cancel
            </Button>
          </div>
        </>
      )}
    </div>
  )
}

type Cer = { id: string | null; name: string; detail: string; answered: boolean; error: string | null; mismatch: boolean }
/** §12.7: the other side's machine (`addr` carries it for a contact ceremony), OS and fingerprint. */
const detailOf = (c: Pick<PairCeremony, 'addr' | 'os_name' | 'peer'>) => [c.addr, c.os_name, fingerprint(c.peer)].filter(Boolean).join(' · ')
/** §7.1: the engine's refusal of a Nearby card whose JWS does not verify against its endpoint id. */
const UNVERIFIED = /unverified card|does not match the advert/
/** `contact_window` calls in call order. The engine serves concurrent requests in any order, so a remounted dialog's
 * close (the previous mount's cleanup) could land after its reopen and leave no window. */
let windowCalls: Promise<unknown> = Promise.resolve()
function windowCall<T>(call: () => Promise<T>): Promise<T> {
  const next = windowCalls.then(call, call)
  windowCalls = next.catch(() => undefined)
  return next
}

/**
 * 183 §7.4, §12.7: Add contact… — opens the contact window (`contact_window(true)`, closed on unmount) and its two tabs:
 * Nearby agents (`contact_nearby`, patched by `contact_nearby` events) and Invite (this window's ticket as text and QR,
 * or paste theirs). A ceremony this side starts, or a stranger's that arrives while the window is open, replaces the
 * tabs with the code until it ends. "Choose what … may reach" calls `onAccess` with the new contact's endpoint id.
 */
export function AddContact({ onClose, onAccess, tab: initialTab = 'nearby' }: { onClose: () => void; onAccess: (peer: string | undefined) => void; tab?: 'nearby' | 'invite' }) {
  const canQuery1 = useQuerySupport(["contact_nearby"])
  const api = useApi()
  const qc = useQueryClient()
  const [tab, setTab] = useState(initialTab)
  const [win, setWin] = useState<ContactWindow | null>(null)
  const [winError, setWinError] = useState<string | null>(null)
  const [paste, setPaste] = useState('')
  const [cer, setCer] = useState<Cer | null>(null)
  const [seen, setSeen] = useState<readonly string[]>([])
  // §7.1: Nearby hints whose card failed verification on click: shown "unverified", Add refused.
  const [unverified, setUnverified] = useState<readonly string[]>([])
  const dial = useRef(0)
  const dialing = useRef(0)
  const cancelRef = useRef<HTMLButtonElement | null>(null)
  // As in the pair dialog (U2-MUST-1): Cancel / Done / Back takes the focus whenever it mounts, unless the user already
  // focused another button in this dialog.
  const cancelMount = useCallback((b: HTMLButtonElement | null) => {
    cancelRef.current = b
    const f = document.activeElement
    if (b && !(f instanceof HTMLButtonElement && b.closest('[role=dialog]')?.contains(f))) b.focus()
  }, [])
  const ceremonies = useQuery<Record<string, PairCeremony>>({ queryKey: queryKeys.contactPair, queryFn: () => ({}), enabled: canQuery1 && (false), staleTime: Infinity }).data
  const nearby = useQuery({ queryKey: queryKeys.contactNearby, queryFn: () => api.contact_nearby(), staleTime: 0, enabled: canQuery1 && (win !== null) })

  const open = useCallback(
    () =>
      windowCall(() => api.contact_window(true)).then(
        (w) => {
          setWin(w)
          setWinError(null)
        },
        (e: unknown) => setWinError(errorText(e)),
      ),
    [api],
  )
  useEffect(() => {
    void open()
    return () => void windowCall(() => api.contact_window(false)).catch(() => undefined)
  }, [api, open])

  useEffect(() => {
    const c = dialing.current ? undefined : Object.values(ceremonies ?? {}).find((c) => c.state === 'waiting' && c.expires_ms > Date.now() && !seen.includes(c.id))
    if (!cer && c) setCer({ id: c.id, name: c.name, detail: detailOf(c), answered: false, error: null, mismatch: false })
  }, [cer, ceremonies, seen])

  const start = (target: ContactTarget, name: string, detail: string) => {
    const token = ++dial.current
    dialing.current++
    setCer({ id: null, name, detail, answered: false, error: null, mismatch: false })
    api.contact_pair(target).then(
      (c) => {
        dialing.current--
        putPair(qc, c, queryKeys.contactPair)
        if (token !== dial.current) {
          setSeen((s) => [...s, c.id])
          void api.contact_answer(c.id, false).catch(() => undefined)
        } else setCer((f) => f && { ...f, id: c.id, name: c.name || f.name, detail: detailOf(c) || f.detail })
      },
      (e: unknown) => {
        dialing.current--
        const why = errorText(e)
        if (target.kind === 'nearby' && UNVERIFIED.test(why)) setUnverified((u) => [...u, target.eid])
        if (token === dial.current) setCer((f) => f && { ...f, error: why })
      },
    )
  }

  const ceremony = cer?.id ? ceremonies?.[cer.id] : undefined
  const view: CeremonyView | null = cer && (cer.mismatch ? 'mismatch' : pairView(ceremony, cer.error, cer.answered))
  const live = view === 'starting' || view === 'code' || view === 'waiting'
  /** Leave the ceremony: refuse it while it is open, never reopen it. */
  const leave = () => {
    if (!cer) return
    if (cer.id === null) dial.current++
    else {
      if (live) void api.contact_answer(cer.id, false).catch(() => undefined)
      setSeen((s) => [...s, cer.id as string])
    }
    setCer(null)
  }
  const accept = () => {
    if (!cer?.id) return
    setCer({ ...cer, answered: true })
    api.contact_answer(cer.id, true).catch((e: unknown) => setCer((f) => f && { ...f, error: errorText(e) }))
  }
  const mismatch = () => {
    if (!cer?.id) return
    void api.contact_answer(cer.id, false).catch(() => undefined)
    setSeen((s) => [...s, cer.id as string])
    setCer({ ...cer, mismatch: true })
  }
  const close = () => {
    leave()
    onClose()
  }
  const connect = () => {
    const v = paste.trim()
    if (!v) return
    start(v.startsWith('theuth-contact:') ? { kind: 'ticket', ticket: v } : { kind: 'card_url', url: v }, v.startsWith('theuth-contact:') ? 'the invite' : v, '')
  }
  // Theuth agents first (verified cards before the rest), then other vendors (§12.7).
  const agents = [...(nearby.data ?? [])].sort((a, b) => Number(!a.eid) - Number(!b.eid) || Number(!a.verified) - Number(!b.verified))

  return (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogContent
        className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-lg"
        aria-describedby={view === 'code' || view === 'waiting' ? LIMITED_ID : undefined}
        onOpenAutoFocus={(e) => {
          if (!cer) return
          e.preventDefault()
          cancelRef.current?.focus()
        }}
      >
        {cer && view ? (
          <CeremonyBody
            name={cer.name}
            detail={cer.detail}
            ceremony={ceremony}
            view={view}
            error={cer.error}
            onAccept={accept}
            onCancel={leave}
            onMismatch={mismatch}
            onBack={leave}
            onDone={close}
            onAccess={() => {
              close()
              onAccess(ceremony?.peer)
            }}
            cancelRef={cancelMount}
          />
        ) : (
          <div className="flex min-w-0 flex-col gap-4">
            <div>
              <DialogTitle className="text-lg font-semibold">Add contact</DialogTitle>
              <p className="text-[13px] text-fg-secondary">Someone else's theuth, or another A2A agent. Both sides check the same 6-digit code.</p>
            </div>
            {winError ? (
              <>
                <p className="text-sm text-fg-secondary">{winError}</p>
                <div className="flex justify-end">
                  <Button variant="outline" onClick={close}>
                    Close
                  </Button>
                </div>
              </>
            ) : (
              <Tabs value={tab} onValueChange={(v) => setTab(v as typeof tab)}>
                <TabsList className="w-full">
                  <TabsTrigger value="nearby">Nearby agents</TabsTrigger>
                  <TabsTrigger value="invite">Invite</TabsTrigger>
                </TabsList>
                <TabsContent value="nearby" className="flex flex-col pt-2">
                  {agents.length === 0 && <Empty>{win ? 'No A2A agent found on this network yet.' : 'Opening the window…'}</Empty>}
                  {agents.map((n) => {
                    const foreign = !n.eid
                    const bad = !!n.eid && unverified.includes(n.eid)
                    const target: ContactTarget | null = bad ? null : n.eid ? { kind: 'nearby', eid: n.eid } : n.card ? { kind: 'card_url', url: n.card } : null
                    return (
                      <div key={`${n.service}|${n.name}|${n.addr}`} className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border-subtle py-2.5 last:border-0">
                        <div className="min-w-0 flex-1 basis-48">
                          <div className="flex flex-wrap items-center gap-1.5 text-sm font-medium">
                            {n.name}
                            {foreign ? (
                              <span className={CHIP}>A2A · HTTPS</span>
                            ) : bad ? (
                              <span className={cn(CHIP, 'text-danger')} title="Its card's signature does not match its key: it cannot be added">
                                unverified
                              </span>
                            ) : (
                              n.verified && <span className={cn(CHIP, 'text-success')}>verified</span>
                            )}
                          </div>
                          <div className="text-[13px] text-fg-tertiary">{[foreign ? `other vendor · ${n.service}` : 'theuth', n.addr].join(' · ')}</div>
                        </div>
                        <Button size="sm" variant="outline" aria-label={`Add ${n.name}`} disabled={!target} onClick={() => target && start(target, n.name, n.addr)}>
                          Add…
                        </Button>
                      </div>
                    )
                  })}
                  <p className="pt-2 text-[13px] text-fg-tertiary">This machine is visible on the network only while this window is open.</p>
                </TabsContent>
                <TabsContent value="invite" className="flex flex-col gap-4 pt-2">
                  {win ? (
                    <div className="flex flex-col items-center gap-3 sm:flex-row sm:items-start">
                      <div className="shrink-0 rounded-lg bg-white p-2">
                        <QRCodeSVG value={win.ticket} size={148} />
                      </div>
                      <div className="flex min-w-0 flex-1 flex-col gap-2">
                        <p className="text-[13px] text-fg-secondary">Send this invite to the other person. It works once, until {new Date(win.expires_ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}, and grants nothing until you both accept the same code.</p>
                        <code className="max-h-20 overflow-y-auto rounded-md bg-layer-2 px-2 py-1.5 font-mono text-xs break-all" aria-label="Invite ticket">
                          {win.ticket}
                        </code>
                        <div className="flex flex-wrap gap-2">
                          <Button size="sm" variant="outline" onClick={() => void navigator.clipboard.writeText(win.ticket).then(() => toast.message('Copied'))}>
                            <Copy /> Copy invite
                          </Button>
                          <Button size="sm" variant="ghost" onClick={() => void open()}>
                            New invite
                          </Button>
                        </div>
                      </div>
                    </div>
                  ) : (
                    <Empty>Creating an invite…</Empty>
                  )}
                  <form
                    className="flex flex-col gap-2 border-t border-border-subtle pt-3"
                    onSubmit={(e) => {
                      e.preventDefault()
                      connect()
                    }}
                  >
                    <label className="text-[13px] font-medium" htmlFor="contact-paste">
                      Or paste theirs
                    </label>
                    <div className="flex gap-2">
                      <Input id="contact-paste" value={paste} onChange={(e) => setPaste(e.target.value)} placeholder="theuth-contact:1.… or a card URL" className="min-w-0 flex-1 font-mono text-base md:text-xs" />
                      <Button type="submit" disabled={!paste.trim()}>
                        Connect
                      </Button>
                    </div>
                  </form>
                </TabsContent>
              </Tabs>
            )}
            <Limited>{PAIRED_RULE}</Limited>
          </div>
        )}
      </DialogContent>
    </Dialog>
  )
}

/** 191: what a grant change says when the engine ignored it (one older than `grant` / `revoke`). */
export const OLD_ENGINE = 'this engine is older than this app; update it'

/** `contact_update` granting (`on`) or revoking export `exp` alone; the answer must show it, so an engine that ignored the
 * patch never reads as success (a blocked contact's answer lists nothing). */
async function changeGrant(api: Api, contact: Contact, exp: string, on: boolean) {
  const c = await api.contact_update(who(contact), { ...NO_PATCH, ...(on ? { grant: [exp] } : { revoke: [exp] }) })
  if (on ? !c.blocked && !c.exports.includes(exp) : c.exports.includes(exp)) throw new Error(OLD_ENGINE)
}

/** Grant `target` to `contact`: its export when one exists, else a new one named from `title` (`a2a_export_put`), then
 * `contact_update(grant)`, which touches that export only (191: never a whole list built from this machine's view, which
 * would take back shares it cannot see). Every share is full (190 D2), so the patch names no mode. */
export function useGrant() {
  const api = useApi()
  return useAction(
    ({ contact, target, title }: { contact: Contact; target: ExportTarget; title: string }) =>
      (async () => {
        const all = await api.a2a_exports()
        const existing = all.find((x) => (x.target.kind === 'agent' ? target.kind === 'agent' && x.target.agent === target.agent : target.kind === 'chat' && x.target.session === target.session))
        const name = existing?.name ?? exportName(title, all.map((x) => x.name))
        if (!existing) await api.a2a_export_put({ name, target, description: title, paused: null })
        if (!contact.exports.includes(name)) await changeGrant(api, contact, name, true)
      })(),
    { keys: [queryKeys.contacts, queryKeys.exports] },
  )
}

/** Stop sharing `exp` with `contact` (`contact_update(revoke)`: that export only). The export stays (no route): the next
 * share of its chat reuses it (191: never removed from this machine's list). */
export function useUngrant() {
  const api = useApi()
  return useAction(({ contact, exp }: { contact: Contact; exp: string }) => changeGrant(api, contact, exp, false), { keys: [queryKeys.contacts, queryKeys.exports] })
}

type Pick_ = { key: string; target: ExportTarget; title: string; group: 'Agents' | 'Chats'; detail: string }

/** §7.6, §12.10: what can be shared — this machine's desk roots and `theuth`, then chats (not archived) with their machine. */
function useShareable(enabled: boolean): Pick_[] {
  const canQuery2 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const agents = useAgents()
  const mesh = useMesh()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canQuery2 && (enabled) })
  const self = mesh.data?.self_name ?? ''
  const agent = (a: string): Pick_ => ({ key: `agent|${a}`, target: { kind: 'agent', agent: a }, title: a, group: 'Agents', detail: 'agent' })
  return [
    agent('theuth'),
    ...(agents.data ?? []).filter((a) => a.desk !== null && self).map((a) => agent(`${a.file.name}@${self}`)),
    ...(sessions.data ?? [])
      .filter((s) => s.parent === null && !s.archived)
      .map((s): Pick_ => ({ key: `chat|${s.id}`, target: { kind: 'chat', session: s.id }, title: chatName(s), group: 'Chats', detail: s.here ? 'this machine' : engineLabel(s.owner, mesh.data?.peers) })),
  ]
}

/**
 * 183 §7.6, §12.10 (D9): Access, under the contacts list — per contact what it may reach (agents and chats, each with its
 * first-contact answer and Paused state), and Share… to pick another agent or chat. `focus` names the contact whose row
 * scrolls into view (188). 191: what it reaches is mesh-wide (a chat on another machine says which); a held contact is
 * listed with its state, its shares and Share… (they apply wherever it is admitted).
 */
export function ContactAccess({ focus = null }: { focus?: string | null }) {
  const contacts = useContacts()
  const exports = useExports()
  return (
    <Section title="Access">
      <p className="text-[13px] text-fg-secondary">Choose, per contact, which agents and chats it may reach, and how each share answers its first message.</p>
      <p className="text-[13px] text-fg-secondary">A new share starts on "Always allow". "Ask first" holds a contact's first message until you press Allow.</p>
      {contacts.isPending && <Empty>Loading…</Empty>}
      {contacts.isError && <Empty>{errorText(contacts.error)}</Empty>}
      {contacts.data?.length === 0 && <Empty>Add a contact above to choose what it may reach.</Empty>}
      {contacts.data?.map((c) => (
        <Access key={c.id || c.name} c={c} exports={exports.data ?? []} focused={c.name === focus && !held(c)} />
      ))}
    </Section>
  )
}

function Access({ c, exports, focused }: { c: Contact; exports: readonly Export[]; focused: boolean }) {
  const row = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (focused) row.current?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }, [focused])
  const api = useApi()
  const [picking, setPicking] = useState(false)
  const [q, setQ] = useState('')
  const grant = useGrant()
  const ungrant = useUngrant()
  const shareable = useShareable(picking)
  const machineOf = useChatMachine()
  const first = useAction((v: { exp: string; answer: FirstContact }) => api.contact_first_answer(who(c), v.exp, v.answer), { keys: [queryKeys.contacts] })
  const granted = new Set(c.exports.flatMap((n) => exports.find((x) => x.name === n) ?? []).map((x) => `${x.target.kind}|${x.target.kind === 'agent' ? x.target.agent : x.target.session}`))
  const options = shareable.filter((p) => !granted.has(p.key) && `${p.title} ${p.detail}`.toLowerCase().includes(q.trim().toLowerCase()))
  const busy = grant.isPending || ungrant.isPending
  return (
    <div ref={row} className="border-b border-border-subtle py-3 last:border-0">
      <div className="min-w-0">
        <div className="text-sm font-medium">
          {c.name}
          {c.blocked && <span className="ml-1.5 rounded-full bg-danger/10 px-1.5 text-xs text-danger">blocked</span>}
        </div>
        <div className="text-[13px] text-fg-secondary">{c.machine}</div>
      </div>
      <div className="mt-2 flex flex-col gap-1.5">
        {held(c) ? (
          <p role="status" className="text-[13px] text-fg-secondary">
            {heldLine(c)}
          </p>
        ) : (
          c.exports.length === 0 && <p className="text-[13px] text-fg-tertiary">{c.name} reaches nothing yet.</p>
        )}
        {c.exports.map((n) => {
          const x = exports.find((e) => e.name === n)
          return (
            <div key={n} className="flex flex-col gap-1.5 rounded-lg bg-layer-2 px-2.5 py-1.5">
              <div className="flex flex-wrap items-center gap-2">
                <div className="flex min-w-0 flex-1 basis-40">
                  <ExportChip name={n} exp={x} on={machineOf(x)} />
                </div>
                {/* A held contact's row carries no first-contact answers here: they are read once it is admitted. */}
                {!held(c) && (
                  <Select value={c.first_contact[n] ?? 'ask'} disabled={first.isPending} onValueChange={(v) => first.mutate({ exp: n, answer: v as FirstContact })}>
                    <SelectTrigger size="sm" className="w-36 shrink-0" aria-label={`First contact from ${c.name} to ${n}`}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {FIRST.map(([v, label]) => (
                        <SelectItem key={v} value={v}>
                          {label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
                <Button variant="ghost" size="icon-sm" aria-label={`Stop sharing ${x?.description || n} with ${c.name}`} disabled={busy} onClick={() => ungrant.mutate({ contact: c, exp: n })}>
                  <X />
                </Button>
              </div>
              <p className="text-xs text-fg-tertiary" title={`The full address is peer:<the name ${c.name} gave you>/${n}`}>
                {c.name} reaches this as …/{n}
              </p>
            </div>
          )
        })}
        {picking ? (
          <div className="flex flex-col gap-1.5 rounded-lg border border-border-subtle p-2">
            <div className="flex gap-2">
              <Input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search agents and chats" aria-label={`Share with ${c.name}`} className="h-8 min-w-0 flex-1 text-base md:text-sm" />
              <Button variant="ghost" size="sm" onClick={() => setPicking(false)}>
                Done
              </Button>
            </div>
            <p className="px-1 text-xs text-fg-secondary">{SHARE_RULE}</p>
            <div className="max-h-64 overflow-y-auto">
              {(['Agents', 'Chats'] as const).map((g) => {
                const rows = options.filter((p) => p.group === g)
                return rows.length === 0 ? null : (
                  <div key={g} role="group" aria-label={g}>
                    <p className="px-1 pt-1.5 text-xs font-medium text-fg-tertiary">{g}</p>
                    {rows.map((p) => (
                      <button
                        key={p.key}
                        type="button"
                        disabled={busy}
                        aria-label={`Share ${p.title} with ${c.name}`}
                        className="flex w-full items-center gap-2 rounded-md px-1.5 py-1.5 text-left text-sm hover:bg-layer-2 disabled:opacity-50 pointer-coarse:min-h-11"
                        onClick={() => grant.mutate({ contact: c, target: p.target, title: p.title })}
                      >
                        {g === 'Agents' ? <Bot className="size-3.5 shrink-0" aria-hidden="true" /> : <MessageSquare className="size-3.5 shrink-0" aria-hidden="true" />}
                        <span className="min-w-0 flex-1 truncate">{p.title}</span>
                        <span className="shrink-0 rounded-[4px] bg-layer-3 px-1 text-[10px] leading-4 text-fg-tertiary">{p.detail}</span>
                      </button>
                    ))}
                  </div>
                )
              })}
              {options.length === 0 && <p className="px-1 py-2 text-[13px] text-fg-tertiary">Nothing else to share.</p>}
            </div>
          </div>
        ) : (
          <div>
            <Button variant="outline" size="sm" onClick={() => setPicking(true)}>
              <Plus /> Share with {c.name}…
            </Button>
          </div>
        )}
      </div>
    </div>
  )
}

/** 183 §12.10: the chat ⋯ menu's Share with contact… (`onShare` opens `ShareDialog` outside the menu) and, when contacts can
 * message the chat, Stop sharing with <them> (a revoke for each of them; the export stays for the next share). */
export function SharingItems({ session, onShare }: { session: string; onShare: () => void }) {
  const api = useApi()
  const sharing = useSharing(session)
  const stop = useAction(
    async (s: Sharing) => {
      for (const c of s.contacts) await changeGrant(api, c, s.exp.name, false)
    },
    { keys: [queryKeys.contacts, queryKeys.exports] },
  )
  return (
    <>
      <DropdownMenuItem onSelect={onShare}>Share with contact…</DropdownMenuItem>
      {sharing && sharing.contacts.length > 0 && (
        <DropdownMenuItem variant="destructive" disabled={stop.isPending} onSelect={() => stop.mutate(sharing)}>
          Stop sharing with {names(sharing.contacts)}
        </DropdownMenuItem>
      )}
    </>
  )
}

/**
 * 183 §12.10: ⋯ Share with contact… — the open chat shared with (or taken back from) each contact. Sharing creates the
 * chat's export on first use; the contact's agents can then send into it and see only what this chat's agent sends back.
 */
export function ShareDialog({ session, title, onClose }: { session: string; title: string; onClose: () => void }) {
  const navigate = useNavigate()
  const contacts = useContacts()
  const exports = useExports()
  const grant = useGrant()
  const ungrant = useUngrant()
  const exp = exports.data?.find((x) => x.target.kind === 'chat' && x.target.session === session)
  const busy = grant.isPending || ungrant.isPending
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      {/* 186: no share button takes focus on open, so Enter never shares by accident. */}
      <DialogContent
        className="sm:max-w-md"
        onOpenAutoFocus={(e) => {
          e.preventDefault()
          ;(e.currentTarget as HTMLElement).focus()
        }}
      >
        <div className="flex flex-col gap-4">
          <div>
            <DialogTitle className="text-lg font-semibold">Share this chat with a contact</DialogTitle>
            <p className="text-[13px] text-fg-secondary">Their agents can send messages into “{title}”. They see only what this chat's agent sends back: no transcript, no live view, no typing.</p>
            <p className="mt-1 text-[13px] text-fg-secondary">{SHARE_RULE}</p>
          </div>
          {exp?.paused && <p className="text-[13px] text-warning">Paused ({exp.paused})</p>}
          <div className="flex flex-col">
            {contacts.isPending && <Empty>Loading…</Empty>}
            {contacts.isError && <Empty>{errorText(contacts.error)}</Empty>}
            {contacts.data?.length === 0 && <Empty>No contacts yet.</Empty>}
            {/* 191: a held contact is offered too, with its state: its shares apply wherever it is admitted. */}
            {contacts.data
              ?.filter((c) => !c.blocked)
              .map((c) => {
                const on = !!exp && c.exports.includes(exp.name)
                return (
                  <div key={c.id || c.name} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border-subtle py-2.5 last:border-0">
                    <div className="min-w-0 flex-1 basis-40">
                      <div className="text-sm font-medium">{c.name}</div>
                      <div className="text-[13px] text-fg-secondary">{c.machine}</div>
                      {held(c) && (
                        <p role="status" className="text-[13px] text-fg-secondary">
                          {heldLine(c)}
                        </p>
                      )}
                    </div>
                    {on && exp ? (
                      <Button size="sm" variant="outline" className="pointer-coarse:min-h-11" disabled={busy} aria-label={`Stop sharing with ${c.name}`} onClick={() => ungrant.mutate({ contact: c, exp: exp.name })}>
                        Shared · Stop
                      </Button>
                    ) : (
                      <Button
                        size="sm"
                        variant="outline"
                        className="pointer-coarse:min-h-11"
                        disabled={busy}
                        aria-label={`Share this chat with ${c.name}`}
                        onClick={() => grant.mutate({ contact: c, target: { kind: 'chat', session }, title })}
                      >
                        Share
                      </Button>
                    )}
                  </div>
                )
              })}
          </div>
          <Limited>You can stop sharing at any time.</Limited>
          <div className="flex flex-wrap justify-end gap-2">
            <Button
              variant="ghost"
              onClick={() => {
                onClose()
                void navigate(toContacts())
              }}
            >
              Contacts…
            </Button>
            <Button variant="outline" onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
