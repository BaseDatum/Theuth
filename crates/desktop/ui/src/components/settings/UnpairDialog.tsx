import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, X } from 'lucide-react'
import { useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { keys } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { UNPAIR_CONTACTS, names, rogueLine, useContacts } from '@/lib/contacts'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { formatAt } from '@/lib/time'
import type { Contact } from '@/rpc/bindings/Contact'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import type { RotationPeer } from '@/rpc/bindings/RotationPeer'
import type { RotationPhase } from '@/rpc/bindings/RotationPhase'
import type { RotationState } from '@/rpc/bindings/RotationState'
import type { UnpairPreview } from '@/rpc/bindings/UnpairPreview'

/** Engine id (or name) → display name, from `mesh_status`; unknown ones as given. */
export type NameOf = (id: string) => string

/** `NameOf` for `s`: this machine, then a peer by engine id; an unknown one is "another machine" (183 R2: never an id). */
export const nameOfIn =
  (s: MeshStatus): NameOf =>
  (id) =>
    id === s.self_id ? s.self_name : (s.peers.find((p) => p.id === id)?.name || 'another machine')

/** A rotation peer by the name the engine recorded, else `nameOf` (183 R2). */
const peerName = (nameOf: NameOf) => (p: { id: string; name: string }): string => p.name.trim() || nameOf(p.id)

/** The removed machines by the names the record keeps (183 R2); "a removed machine" when none is known, never an id. */
export const removedNames = (s: RotationState, nameOf: NameOf): string[] =>
  s.removed.map((id, i) => s.removed_names[i]?.trim() || (nameOf(id) === 'another machine' ? 'a removed machine' : nameOf(id)))

/** 183 §12.9: the confirm step — the secret names to replace, the lines that apply, the D11 checkbox. 183 R5: a failed
 * preview offers Retry; after a failed retry, "Unpair without the list" behind its own confirm, so a lost machine an
 * older engine cannot preview can still be unpaired. */
export function UnpairBody({
  name,
  preview,
  previewError,
  onRetryPreview,
  previewRetrying,
  previewFailedTwice,
  nameOf,
  reset,
  onReset,
  error,
  busy,
  onCancel,
  onConfirm,
  contacts = [],
}: {
  name: string
  preview: UnpairPreview | undefined
  previewError: string | null
  onRetryPreview: () => void
  previewRetrying: boolean
  /** The preview failed again after a Retry. */
  previewFailedTwice: boolean
  nameOf: NameOf
  reset: boolean
  onReset: (on: boolean) => void
  error: string | null
  busy: boolean
  onCancel: () => void
  onConfirm: () => void
  /** 191 D8: the mesh's contacts, told this machine left; empty → no contact lines. */
  contacts?: readonly Pick<Contact, 'name'>[]
}) {
  const p = preview
  const signIns = p?.sign_ins.map((ns) => ns.replace(/^oauth\//, '')) ?? []
  const [blind, setBlind] = useState(false)
  return (
    <div className="flex flex-col gap-4">
      <DialogTitle className="text-lg font-semibold">Unpair {name}?</DialogTitle>
      <p className="text-sm text-fg-secondary">
        The machines that stay get a new vault key and re-encrypt every secret with it. {name} keeps every secret it already had: nothing can take those back.
      </p>
      {!p && !previewError && <p className="text-sm text-fg-tertiary">Reading what {name} had…</p>}
      {previewError && !p && (
        <div className="flex flex-col gap-2">
          <div className="flex items-start gap-3">
            <p className="min-w-0 flex-1 text-sm text-danger">Could not read what {name} had: {previewError}</p>
            <Button size="sm" variant="outline" onClick={onRetryPreview} disabled={previewRetrying || busy}>
              {previewRetrying ? 'Retrying…' : 'Retry'}
            </Button>
          </div>
          {previewFailedTwice && !blind && (
            <div className="flex justify-end">
              <Button size="sm" variant="outline" onClick={() => setBlind(true)} disabled={busy}>
                Unpair without the list…
              </Button>
            </div>
          )}
          {blind && (
            <div role="alert" className="flex flex-col gap-2 rounded-lg border border-danger-border/40 bg-danger-bg px-3 py-2 text-[13px]">
              <p className="text-danger">
                The list of what {name} had could not be read. Unpairing without it, you will not see which secrets and sign-ins to replace at their providers.
              </p>
              <div className="flex justify-end gap-2">
                <Button size="sm" variant="outline" onClick={() => setBlind(false)} disabled={busy}>
                  Back
                </Button>
                <Button size="sm" variant="destructive" onClick={onConfirm} disabled={busy}>
                  {busy ? 'Unpairing…' : 'Unpair without the list'}
                </Button>
              </div>
            </div>
          )}
        </div>
      )}
      {p && (
        <section>
          <h3 className="text-sm font-medium">Replace these at their providers if {name} is lost</h3>
          {p.secrets.length === 0 && signIns.length === 0 ? (
            <p className="mt-1 text-sm text-fg-tertiary">{name} had no secrets.</p>
          ) : (
            <ul className="mt-2 max-h-48 overflow-y-auto rounded-lg border border-border-subtle bg-layer-2 px-3 py-2 text-[13px]">
              {p.secrets.map((s) => (
                <li key={s} className="truncate py-0.5 font-mono">
                  {s}
                </li>
              ))}
              {signIns.length > 0 && <li className="py-0.5">Sign in again: {signIns.join(', ')}</li>}
            </ul>
          )}
        </section>
      )}
      {p && (p.offline.length > 0 || p.old_build.length > 0 || p.devices > 0) && (
        <ul className="flex flex-col gap-1 text-[13px] text-fg-secondary">
          {p.offline.map((m) => (
            <li key={`off-${m}`}>{nameOf(m)} is offline: it gets the new key when it reconnects</li>
          ))}
          {p.old_build.map((m) => (
            <li key={`old-${m}`} className="text-warning">
              {nameOf(m)} runs an older theuth: it cannot read secrets after this until it is upgraded
            </li>
          ))}
          {p.devices > 0 && (
            <li>
              Phones paired to this mesh trust a key {name} knows: re-pair them if {name} is lost
            </li>
          )}
        </ul>
      )}
      {contacts.length > 0 && (
        <div className="flex flex-col gap-1 text-[13px] text-fg-secondary">
          <p>{UNPAIR_CONTACTS}</p>
          <p>{`If ${name} was lost or stolen: ${rogueLine(names(contacts))}`}</p>
        </div>
      )}
      <label className="flex items-start gap-2 text-sm">
        <input type="checkbox" className="mt-0.5 size-4 accent-[var(--primary)]" checked={reset} onChange={(e) => onReset(e.currentTarget.checked)} />
        <span>
          Also reset phone and iPad pairing
          {reset && <span className="mt-0.5 block text-[13px] text-fg-secondary">Every phone and iPad paired to this mesh must scan the pairing QR again (Settings › Network › Devices).</span>}
        </span>
      </label>
      {error && <p className="text-sm text-danger">{error}</p>}
      <div className="flex justify-end gap-2">
        <Button variant="outline" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
        <Button variant="destructive" onClick={onConfirm} disabled={busy || !p}>
          {busy ? 'Unpairing…' : 'Unpair and rotate key'}
        </Button>
      </div>
    </div>
  )
}

const STEPS: readonly [RotationPhase, string][] = [
  ['minting', 'New key'],
  ['sweeping', 'Re-encrypt'],
  ['waiting', 'Every machine'],
  ['done', 'Old key destroyed'],
]

/** One remaining machine's line in the progress list. */
export function peerLine(p: RotationPeer): string {
  if (p.has_key) return 'has the new key'
  if (p.old_build) return 'runs an older theuth: upgrade it to read secrets'
  if (!p.online) return 'offline: gets the key when it reconnects'
  return 'pending'
}

/** The §12.9 banner text for `s`. */
export function rotationHeadline(s: RotationState, nameOf: NameOf, now = Date.now()): string {
  const pending = s.peers.filter((p) => !p.has_key).map(peerName(nameOf))
  const withKey = s.peers.length - pending.length
  switch (s.phase) {
    case 'minting':
    case 'sweeping':
      return `Rotating vault key · ${s.rows_done} of ${s.rows_total} secrets re-encrypted · ${withKey} of ${s.peers.length} machines ${withKey === 1 ? 'has' : 'have'} the new key`
    case 'waiting':
      return `New vault key in use · ${pending.length === 0 ? 'every machine has it' : `${pending.join(', ')} ${pending.length === 1 ? 'gets it when it reconnects' : 'get it when they reconnect'}`} · old key destroyed after the last machine re-encrypts`
    case 'done':
      return `Vault key rotated (started ${formatAt(s.started_ms, now)}) · ${s.old_destroyed ? 'old key destroyed' : 'old key destroyed once nothing uses it'}`
    case 'failed':
      return `Vault key rotation stopped: ${s.error ?? 'unknown error'}`
  }
}

/**
 * 183 §12.9: rotation progress — the phases, the bar, the machines done and pending (older builds and offline ones
 * named), stuck rows, and the error with Retry. `state` `null`: no rotation reported (an engine without it, or
 * before the first); `error` is a failed `vault_rotate`.
 */
export function RotationProgress({
  state: s,
  nameOf,
  error,
  retrying,
  onRetry,
  onDismiss,
  now = Date.now(),
}: {
  state: RotationState | null
  nameOf: NameOf
  error: string | null
  retrying: boolean
  onRetry: () => void
  onDismiss?: () => void
  now?: number
}) {
  const failed = error ?? (s?.phase === 'failed' ? (s.error ?? 'unknown error') : null)
  const retry = (
    <Button size="sm" variant="outline" onClick={onRetry} disabled={retrying}>
      {retrying ? 'Retrying…' : s === null && !error ? 'Rotate now' : 'Retry'}
    </Button>
  )
  if (failed) {
    return (
      <div className="flex flex-col gap-2 rounded-xl border border-danger-border/40 bg-danger-bg px-4 py-3 text-sm">
        <div className="flex items-start gap-3">
          <p role="status" className="min-w-0 flex-1 text-danger">
            Vault key rotation stopped: {failed}
          </p>
          {retry}
        </div>
        <p className="text-[13px] text-fg-secondary">The old key stays in use and nothing is lost.</p>
      </div>
    )
  }
  if (!s) {
    return (
      <div className="flex items-start gap-3 rounded-xl border border-border-subtle bg-layer-2 px-4 py-3 text-sm">
        <p role="status" className="min-w-0 flex-1 text-fg-secondary">No vault-key rotation reported yet. The old key is still in use.</p>
        {retry}
      </div>
    )
  }
  const at = STEPS.findIndex(([p]) => p === s.phase)
  const pct = s.rows_total > 0 ? Math.round((100 * s.rows_done) / s.rows_total) : s.phase === 'minting' ? 0 : 100
  return (
    <div className="flex flex-col gap-3 rounded-xl border border-border-subtle bg-layer-2 px-4 py-3 text-sm">
      <div className="flex items-start gap-3">
        <p role="status" className="min-w-0 flex-1">{rotationHeadline(s, nameOf, now)}</p>
        {s.phase === 'done' && onDismiss && (
          <Button size="icon-sm" variant="ghost" aria-label="Dismiss" onClick={onDismiss}>
            <X />
          </Button>
        )}
      </div>
      <ol className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
        {STEPS.map(([p, label], i) => (
          <li key={p} className={cn('flex items-center gap-1', i < at || s.phase === 'done' ? 'text-success' : i === at ? 'font-semibold' : 'text-fg-tertiary')}>
            {(i < at || s.phase === 'done') && <Check className="size-3" aria-hidden="true" />}
            {label}
          </li>
        ))}
      </ol>
      {(s.phase === 'minting' || s.phase === 'sweeping') && (
        <div className="h-1.5 overflow-hidden rounded-full bg-layer-3" role="progressbar" aria-label="Re-encrypting secrets" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
          <div className="h-full bg-primary transition-[width]" style={{ width: `${pct}%` }} />
        </div>
      )}
      {s.removed.length > 0 && <p className="text-[13px] text-fg-tertiary">After unpairing {removedNames(s, nameOf).join(', ')}</p>}
      {s.peers.length > 0 && (
        <ul className="flex flex-col gap-0.5 text-[13px]">
          {s.peers.map((p) => (
            <li key={p.id} className="flex items-center gap-2">
              <span className={cn('size-1.5 shrink-0 rounded-full', p.has_key ? 'bg-success' : p.old_build ? 'bg-warning' : 'ring-1 ring-fg-tertiary ring-inset')} aria-hidden="true" />
              <span className="font-medium">{peerName(nameOf)(p)}</span>
              <span className={p.old_build && !p.has_key ? 'text-warning' : 'text-fg-secondary'}>{peerLine(p)}</span>
            </li>
          ))}
        </ul>
      )}
      {s.rows_stuck.length > 0 && (
        <p className="text-[13px] text-warning">
          {s.rows_stuck.length} {s.rows_stuck.length === 1 ? 'secret' : 'secrets'} could not be re-encrypted: {s.rows_stuck.join(', ')} · set {s.rows_stuck.length === 1 ? 'it' : 'them'} again
        </p>
      )}
    </div>
  )
}

/** `vault_rotation()` (kept current by `SecretsRotation`) and `vault_rotate()` for Retry / Rotate vault key. */
export function useRotation() {
  const api = useApi()
  const qc = useQueryClient()
  const state = useQuery({ queryKey: queryKeys.vaultRotation, queryFn: () => api.vault_rotation() })
  const rotate = useMutation({ mutationFn: () => api.vault_rotate(), onSuccess: (s) => qc.setQueryData(queryKeys.vaultRotation, s) })
  return { state, rotate }
}

/** The Network page's banner: shown while a rotation is reported (dismissable once done) or a manual rotate failed. */
export function RotationBanner({ nameOf }: { nameOf: NameOf }) {
  const { state, rotate } = useRotation()
  const [dismissed, setDismissed] = useState<string | null>(null)
  const s = state.data ?? null
  const error = rotate.error ? errorText(rotate.error) : null
  if (!error && (!s || (s.phase === 'done' && dismissed === s.key.id))) return null
  return (
    <div className="mb-6">
      <RotationProgress state={s} nameOf={nameOf} error={error} retrying={rotate.isPending} onRetry={() => rotate.mutate()} onDismiss={() => s && setDismissed(s.key.id)} />
    </div>
  )
}

/**
 * 183 §12.9 (D7, D11): ⋯ Unpair. Confirm with the preview, then `mesh_remove(id, reset_devices)`; on success the dialog
 * turns into the rotation's progress. A refusal (resetting devices is not built yet, say) stays on the confirm step.
 */
export function UnpairDialog({ id, name, nameOf, onClose }: { id: string; name: string; nameOf: NameOf; onClose: () => void }) {
  const api = useApi()
  const qc = useQueryClient()
  // Not under `mesh`: the unpair invalidates that, and the removed machine's preview must not refetch.
  const preview = useQuery({ queryKey: ['unpair-preview', id], queryFn: () => api.mesh_unpair_preview(id), gcTime: 0, refetchOnMount: 'always' })
  const [reset, setReset] = useState(false)
  const remove = useMutation({
    mutationFn: () => api.mesh_remove(id, reset),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: keys.mesh })
      void qc.invalidateQueries({ queryKey: queryKeys.vaultRotation })
    },
  })
  const { state, rotate } = useRotation()
  const contacts = useContacts().data ?? []
  return (
    <Dialog open onOpenChange={(o) => !o && !remove.isPending && onClose()}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-lg" aria-describedby={undefined}>
        {remove.isSuccess ? (
          <div className="flex flex-col gap-4">
            <DialogTitle className="text-lg font-semibold">Unpaired {name}</DialogTitle>
            <RotationProgress
              state={state.data ?? null}
              nameOf={(x) => (x === id ? name : nameOf(x))}
              error={rotate.error ? errorText(rotate.error) : null}
              retrying={rotate.isPending}
              onRetry={() => rotate.mutate()}
            />
            <div className="flex justify-end">
              <Button onClick={onClose}>Close</Button>
            </div>
          </div>
        ) : (
          <UnpairBody
            name={name}
            preview={preview.data}
            previewError={preview.error ? errorText(preview.error) : null}
            onRetryPreview={() => void preview.refetch()}
            previewRetrying={preview.isFetching}
            previewFailedTwice={preview.errorUpdateCount >= 2}
            nameOf={nameOf}
            reset={reset}
            onReset={setReset}
            error={remove.error ? errorText(remove.error) : null}
            busy={remove.isPending}
            onCancel={onClose}
            onConfirm={() => remove.mutate()}
            contacts={contacts}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}
