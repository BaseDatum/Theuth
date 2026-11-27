import { useQuery, useQueryClient } from '@tanstack/react-query'
import { TriangleAlert } from 'lucide-react'
import { useState } from 'react'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { useMesh } from '@/components/tasks/queries'
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
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { notify } from '@/lib/notify'
import { cn } from '@/lib/utils'
import { Unavailable } from '@/rpc/capabilities'
import { isUnsupported, type UnlockDevice, type VaultMethod, type VaultMethodKind, type VaultState, type VaultStatus, useVaultApi, vaultKeys } from './api'
import { PasswordForm } from './PasswordForm'
import { PasswordUnlockSheet } from './PasswordUnlockSheet'
import { MIN_PASSWORD } from './strength'

/** A sealed machine is re-read this often, so a device unlocking it elsewhere shows up here. */
const SEALED_REFRESH_MS = 5_000

const STATE: Record<VaultState, { text: string; className: string }> = {
  unsealed: { text: 'Unsealed', className: 'border-success-border bg-success-bg text-success' },
  sealed: { text: 'Sealed', className: 'border-warning-border bg-warning-bg text-warning' },
  unavailable: { text: 'Unavailable', className: 'border-danger-border bg-danger-bg text-danger' },
}

const UNSEALED_BY: Record<VaultMethodKind | 'memory', string> = {
  keychain: 'Unlocked with this machine’s keychain',
  password: 'Unlocked with the master password',
  device: 'Unlocked from another device',
  file: 'Unlocked with the key file on disk',
  memory: 'Unlocked earlier in this run',
}

const KIND: Record<VaultMethodKind, string> = {
  keychain: 'OS keychain',
  password: 'Master password',
  device: 'Unlock device',
  file: 'Unprotected key file',
}

const day = (ms: number) => new Date(ms).toLocaleDateString()

/** One machine of the vault page. */
type Machine = { engine: string | null; name: string; offline: boolean }

/**
 * 225: Settings › Security › Vault. Each machine's vault (the connected engine, then every paired peer through
 * `vault_status(peer)`), its unlock methods and master password, and the machines this device unlocks.
 */
export function VaultSettings() {
  const api = useVaultApi()
  const mesh = useMesh()
  const self = useQuery({ queryKey: vaultKeys.status(null), queryFn: () => api.status(null), retry: false })
  if (self.isError && isUnsupported(self.error)) return <Unavailable />

  const m = mesh.data
  const machines: Machine[] = [
    { engine: null, name: m?.self_name ?? 'This machine', offline: false },
    ...(m?.peers ?? []).map((p) => ({ engine: p.id, name: p.name, offline: !p.reachable })),
  ]
  // Who can become an unlock device of a machine: every other machine of the mesh, by mesh id.
  const everyone = m ? [{ id: m.self_id, name: m.self_name }, ...m.peers.map((p) => ({ id: p.id, name: p.name }))] : []
  const names = new Map(everyone.map((e) => [e.id, e.name]))
  return (
    <>
      <p className="mb-6 rounded-lg border border-border-subtle px-3 py-2 text-[13px] text-fg-secondary">
        Each machine’s secrets are sealed by its own vault key, stored only wrapped by the unlock methods below.{' '}
        <span className="font-medium text-foreground">There is no recovery. If you lose every unlock method, the secrets on that machine are gone.</span>
      </p>
      {machines.map((machine) => (
        <MachineVault key={machine.engine ?? ''} machine={machine} owners={everyone.filter((e) => e.id !== (machine.engine ?? m?.self_id))} names={names} />
      ))}
      <UnlockDevices />
    </>
  )
}

function MachineVault({ machine, owners, names }: { machine: Machine; owners: { id: string; name: string }[]; names: ReadonlyMap<string, string> }) {
  const { engine, offline } = machine
  const api = useVaultApi()
  const qc = useQueryClient()
  const q = useQuery({
    queryKey: vaultKeys.status(engine),
    queryFn: () => api.status(engine),
    enabled: !offline,
    retry: false,
    refetchInterval: (query) => (query.state.data?.state === 'sealed' ? SEALED_REFRESH_MS : false),
  })
  const put = (s: VaultStatus) => qc.setQueryData(vaultKeys.status(engine), s)
  const seal = useAction(() => api.seal(engine), { onSuccess: put })
  const add = useAction((pw: string) => api.addPassword(engine, pw), { onSuccess: put })
  const change = useAction(({ old, next }: { old: string; next: string }) => api.changePassword(engine, old, next), { onSuccess: put })
  const remove = useAction((id: string) => api.removeMethod(engine, id), { onSuccess: put })
  const addDevice = useAction((owner: string) => api.addDevice(engine, owner), {
    onSuccess: (s, owner) => {
      put(s)
      notify.success(`Asked ${names.get(owner) ?? 'that machine'} to unlock ${machine.name}; allow it there if it asks`)
    },
  })

  const [form, setForm] = useState<'add' | 'change' | null>(null)
  const [unlocking, setUnlocking] = useState(false)
  const [sealing, setSealing] = useState(false)
  const [removing, setRemoving] = useState<VaultMethod | null>(null)
  const [owner, setOwner] = useState<string | undefined>(undefined)

  const s = q.data
  const name = s?.name ?? machine.name
  const state = s && STATE[s.state]
  const only = s?.methods.length === 1
  const free = owners.filter((o) => !s?.methods.some((method) => method.kind === 'device' && method.id === o.id))

  return (
    <Section title={name} actions={state && <Badge variant="outline" className={state.className}>{state.text}</Badge>}>
      {offline && <Empty>{name} is offline; its vault shows here once it is back.</Empty>}
      {q.isError && <p className="py-3 text-sm text-danger">Could not read {name}’s vault: {errorText(q.error)}</p>}
      {!offline && q.isPending && <Empty>Reading {name}’s vault…</Empty>}
      {s && (
        <>
          {s.warning && (
            <div role="alert" className="my-3 flex items-start gap-2 rounded-lg border border-danger-border bg-danger-bg px-3 py-2 text-sm font-medium text-danger">
              <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
              <span>{s.warning}</span>
            </div>
          )}
          <Row
            label="Vault"
            description={
              s.state === 'unsealed'
                ? s.unsealed_by && UNSEALED_BY[s.unsealed_by]
                : [s.error, s.asking.length > 0 && `Asking ${s.asking.map((id) => names.get(id) ?? id.slice(0, 8)).join(', ')} to unlock it`].filter(Boolean).join(' · ') || null
            }
          >
            {s.state === 'sealed' && s.password_set && (
              <Button size="sm" onClick={() => setUnlocking(true)}>
                Unlock with password
              </Button>
            )}
            {s.state === 'unsealed' && (
              <Button size="sm" variant="outline" disabled={seal.isPending} onClick={() => setSealing(true)}>
                Seal now
              </Button>
            )}
          </Row>

          <div className="border-b border-border-subtle py-4">
            <div className="text-sm">Unlock methods</div>
            <div className="mt-0.5 text-[13px] text-fg-secondary">Any one of these unlocks the vault. The last one cannot be removed.</div>
            <ul className="mt-2 flex flex-col">
              {s.methods.map((method) => (
                <li key={method.id} className="flex items-center justify-between gap-4 py-1.5">
                  <div className="min-w-0">
                    <div className={cn('text-sm', method.kind === 'file' && 'text-danger')}>{method.label}</div>
                    <div className="text-xs text-fg-tertiary">
                      {KIND[method.kind]} · added {day(method.created_ms)}
                    </div>
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    aria-label={`Remove ${method.label}`}
                    title={only ? 'The only unlock method cannot be removed' : undefined}
                    disabled={only || remove.isPending}
                    onClick={() => setRemoving(method)}
                  >
                    Remove
                  </Button>
                </li>
              ))}
            </ul>
          </div>

          <Row
            wide={form !== null}
            label="Master password"
            description={
              s.password_set
                ? 'Unlocks this vault from anywhere you can reach it; derived on the machine itself.'
                : s.state === 'unsealed'
                  ? `Lets you unlock ${name} with a password after a restart. At least ${MIN_PASSWORD} characters.`
                  : `Unlock ${name} first to add one.`
            }
          >
            {form ? (
              <PasswordForm
                mode={form}
                name={name}
                onSubmit={(next, old) => (form === 'add' ? add.mutateAsync(next) : change.mutateAsync({ old, next }))}
                onDone={() => {
                  notify.success(form === 'add' ? `Master password added to ${name}` : `Master password of ${name} changed`)
                  setForm(null)
                }}
                onCancel={() => setForm(null)}
              />
            ) : s.password_set ? (
              <Button size="sm" variant="outline" onClick={() => setForm('change')}>
                Change password
              </Button>
            ) : (
              <Button size="sm" variant="outline" disabled={s.state !== 'unsealed'} onClick={() => setForm('add')}>
                Add password
              </Button>
            )}
          </Row>

          {free.length > 0 && (
            <Row label="Unlock from another machine" description={`After a restart, ${name} asks that machine for its vault key; you allow it there.`}>
              <Select value={owner} onValueChange={setOwner}>
                <SelectTrigger size="sm" className="w-40" aria-label={`Unlock device for ${name}`}>
                  <SelectValue placeholder="Choose a machine" />
                </SelectTrigger>
                <SelectContent>
                  {free.map((o) => (
                    <SelectItem key={o.id} value={o.id}>
                      {o.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button size="sm" variant="outline" disabled={!owner || addDevice.isPending} onClick={() => owner && addDevice.mutate(owner)}>
                Add
              </Button>
            </Row>
          )}

          <PasswordUnlockSheet engine={engine} name={name} open={unlocking} onOpenChange={setUnlocking} />
          <AlertDialog open={sealing} onOpenChange={setSealing}>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Seal {name} now?</AlertDialogTitle>
                <AlertDialogDescription>
                  Its secrets stay locked until you unlock it again{s.password_set ? ' with the master password' : ''} or from an unlock device. Everything else keeps working.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={() => seal.mutate(undefined)}>Seal now</AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
          <AlertDialog open={removing !== null} onOpenChange={(o) => !o && setRemoving(null)}>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Remove {removing?.label}?</AlertDialogTitle>
                <AlertDialogDescription>
                  {name}’s vault can no longer be unlocked this way. Keep at least one method you can use: there is no recovery.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction
                  onClick={() => {
                    if (removing) remove.mutate(removing.id)
                    setRemoving(null)
                  }}
                >
                  Remove
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </>
      )}
    </Section>
  )
}

/** 225: the sealed machines this device releases a vault key to, from `vault_unlock_devices`. */
function UnlockDevices() {
  const api = useVaultApi()
  const devices = useQuery({ queryKey: vaultKeys.devices, queryFn: () => api.unlockDevices(), retry: false })
  const setAuto = useAction(({ node, auto }: { node: string; auto: boolean }) => api.setAutoUnlock(node, auto), { keys: [vaultKeys.devices] })
  const forget = useAction((node: string) => api.forgetNode(node), { keys: [vaultKeys.devices] })
  const [forgetting, setForgetting] = useState<UnlockDevice | null>(null)
  const list = devices.data ?? []
  return (
    <Section title="Machines this device unlocks">
      {devices.isError && <p className="py-3 text-sm text-danger">{errorText(devices.error)}</p>}
      {devices.isSuccess && list.length === 0 && <Empty>No machine asks this device to unlock it.</Empty>}
      {list.map((d) => (
        <div key={d.node} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border-subtle py-3 last:border-0">
          <div className="min-w-0 flex-1 basis-48">
            <div className="text-sm">{d.name}</div>
            <div className="text-[13px] text-fg-secondary">
              Since {day(d.enrolled_ms)} · {d.auto ? 'unlocks without asking' : 'asks you each time it restarts'}
            </div>
          </div>
          <label className="flex items-center gap-2 text-[13px]">
            <Switch checked={d.auto} disabled={setAuto.isPending} aria-label={`Unlock ${d.name} automatically`} onCheckedChange={(auto) => setAuto.mutate({ node: d.node, auto })} />
            Unlock automatically
          </label>
          <Button size="sm" variant="outline" aria-label={`Forget ${d.name}`} disabled={forget.isPending} onClick={() => setForgetting(d)}>
            Forget
          </Button>
        </div>
      ))}
      <AlertDialog open={forgetting !== null} onOpenChange={(o) => !o && setForgetting(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Forget {forgetting?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              This device stops unlocking {forgetting?.name}. If it has no other unlock method, its secrets cannot be unlocked after its next restart.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (forgetting) forget.mutate(forgetting.node)
                setForgetting(null)
              }}
            >
              Forget
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Section>
  )
}
