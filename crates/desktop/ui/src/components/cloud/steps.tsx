// 224: the cloud-node wizard's screens, presentational only (props in, callbacks out): `CloudNodeWizard` owns the state
// and the RPCs, the fixture and the tests render these directly. Layout follows the approved mockup
// (docs/design/224-hosted-nodes.md): the phone's five screens and the desktop dialog's provider grid / 3-column configure.

import { Check, ChevronDown, Loader2 } from 'lucide-react'
import type { ReactNode } from 'react'
import { Seg, Tag } from '@/components/costs/ui'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { cn } from '@/lib/utils'
import { BRAND, approxUsd, billedLine, costLine, createLabel, etaText, flag, k8sLine, minPrice, offered, type StageRow } from './format'
import type { K8sInfo, K8sOptions, NodeSpec, ProviderInfo, ProviderKind, Quote, Region } from './types'

export type WizardEntry = 'first-run' | 'add-to-mesh' | 'desktop-dialog'

// ---- pieces ----

export function ProviderBadge({ kind, mark, className }: { kind: ProviderKind; mark?: string; className?: string }) {
  return (
    <span aria-hidden className={cn('grid size-7 shrink-0 place-items-center rounded-[7px] text-xs font-bold text-white', className)} style={{ background: BRAND[kind].bg }}>
      {mark ?? BRAND[kind].mark}
    </span>
  )
}

/** The mockup's `.card`; `selected` is its 2 px primary border (drawn as a ring, so nothing shifts). */
export function Card({ selected, dashed, className, children }: { selected?: boolean; dashed?: boolean; className?: string; children: ReactNode }) {
  return <div className={cn('rounded-xl border border-border bg-card px-3 py-2.5', selected && 'border-primary ring-1 ring-primary', dashed && 'border-dashed', className)}>{children}</div>
}

function Muted({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn('text-xs leading-snug text-fg-secondary', className)}>{children}</div>
}

function Chip({ children }: { children: ReactNode }) {
  return <span className="mr-1 inline-block rounded-full bg-layer-2 px-2 py-0.5 text-[11px] text-foreground">{children}</span>
}

function Title({ children, className }: { children: ReactNode; className?: string }) {
  return <h2 className={cn('mt-1 text-[19px] leading-tight font-semibold', className)}>{children}</h2>
}

/** The note line with the primary rule on its left. */
export function Note({ children, className }: { children: ReactNode; className?: string }) {
  return <p className={cn('border-l-[3px] border-primary pl-2 text-xs leading-snug text-fg-secondary', className)}>{children}</p>
}

/** The phone screens' bottom button: full width, tall enough for a thumb. */
function Primary({ children, disabled, onClick, busy }: { children: ReactNode; disabled?: boolean; onClick?: () => void; busy?: boolean }) {
  return (
    <Button className="h-11 w-full rounded-[10px] text-[15px] font-semibold" disabled={disabled || busy} onClick={onClick}>
      {busy && <Loader2 className="animate-spin" />}
      {children}
    </Button>
  )
}

// ---- ① choose (phone first run) ----

export type Choice = 'cloud' | 'local'

export function ChooseStep({ fromPrice, choice, onChoice, onContinue }: { fromPrice: string | null; choice: Choice; onChoice: (c: Choice) => void; onContinue: () => void }) {
  return (
    <>
      <Title>Where should theuth run?</Title>
      <button type="button" aria-pressed={choice === 'cloud'} onClick={() => onChoice('cloud')} className="text-left">
        <Card selected={choice === 'cloud'}>
          <div className="text-[15px] font-semibold">☁︎ An always-on cloud node</div>
          <ul className="my-1.5 list-disc pl-4 text-[13px] leading-snug">
            <li className="my-1">Keeps working when your phone sleeps or is offline</li>
            <li className="my-1">Runs long tasks, schedules and routines all night</li>
            <li className="my-1">Full tools: shell, git, browser, local MCP servers</li>
            <li className="my-1">Your phone, laptop and iPad all share one memory</li>
          </ul>
          {fromPrice && <Chip>{fromPrice}</Chip>}
          <Chip>your own account</Chip>
        </Card>
      </button>
      <button type="button" aria-pressed={choice === 'local'} onClick={() => onChoice('local')} className="text-left">
        <Card selected={choice === 'local'}>
          <div className="text-[15px] font-semibold">📱 Just this iPhone</div>
          <Muted className="mt-0.5">Free. Pauses when the app closes. Tools limited by iOS.</Muted>
        </Card>
      </button>
      <div className="mt-auto flex flex-col gap-2 pt-2">
        <Primary onClick={onContinue}>Continue</Primary>
        <Muted className="text-center">You can add a cloud node later</Muted>
      </div>
    </>
  )
}

// ---- ② provider ----

/** The line under a provider's name. */
export function providerDetail(p: ProviderInfo, phone: boolean, k8sContexts: number | null): string {
  if (!p.available) return 'coming soon'
  if (p.provider === 'kubernetes') {
    const n = k8sContexts ?? p.discovered.length
    return `${n} context${n === 1 ? '' : 's'} in ~/.kube/config`
  }
  const found = !phone ? p.discovered[0] : undefined
  if (found) return `Found ${found.label}${found.detail ? ` · ${found.detail}` : ''}`
  if (p.connected) return p.source === 'oauth' ? `Signed in with ${p.label}` : 'Connected'
  return p.oauth ? `Sign in with ${p.label}` : 'Paste an API token'
}

/** One provider card: badge, name (+ beta), the detail line, and on the right the price / `✓ connected` / Use it. */
export function ProviderCard({
  p,
  phone,
  selected,
  k8sContexts,
  onSelect,
  onUse,
  using,
}: {
  p: ProviderInfo
  phone: boolean
  selected: boolean
  k8sContexts: number | null
  onSelect: (k: ProviderKind) => void
  onUse?: (p: ProviderInfo, id: string) => void
  using?: boolean
}) {
  const price = minPrice(p)
  const found = !phone && !p.connected ? p.discovered[0] : undefined
  let right: ReactNode
  if (!phone && p.connected && p.provider !== 'kubernetes') right = <span className="text-success">✓ connected</span>
  else if (p.provider === 'kubernetes') right = 'uses your cluster'
  else if (found && onUse && p.available)
    right = (
      <Button
        size="xs"
        variant="outline"
        disabled={using}
        onClick={(e) => {
          e.stopPropagation()
          onUse(p, found.id)
        }}
      >
        {using && <Loader2 className="animate-spin" />}
        Use it
      </Button>
    )
  else if (price !== null)
    right = (
      <>
        {approxUsd(price)}/mo
        <br />2 GB
      </>
    )
  return (
    <div
      role="radio"
      aria-checked={selected}
      aria-disabled={!p.available || undefined}
      tabIndex={p.available ? 0 : -1}
      data-provider={p.provider}
      onClick={() => p.available && onSelect(p.provider)}
      onKeyDown={(e) => {
        if (p.available && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault()
          onSelect(p.provider)
        }
      }}
      className={cn('rounded-xl outline-none focus-visible:ring-2 focus-visible:ring-ring/50', p.available ? 'cursor-pointer' : 'cursor-not-allowed opacity-55')}
    >
      <Card selected={selected} className="flex items-center gap-2.5">
        <ProviderBadge kind={p.provider} />
        <div className="min-w-0">
          <div className="flex items-center gap-1.5 text-sm font-semibold">
            {p.provider === 'kubernetes' && !phone ? 'My Kubernetes' : p.label}
            {p.beta && <Tag tone="info">beta</Tag>}
          </div>
          <Muted className="truncate">{providerDetail(p, phone, k8sContexts)}</Muted>
        </div>
        <div className="ml-auto shrink-0 text-right text-[11px] leading-tight text-fg-secondary">{right}</div>
      </Card>
    </div>
  )
}

export function ProviderCards({
  providers,
  phone,
  selected,
  k8sContexts,
  onSelect,
  onUse,
  using,
  grid,
}: {
  providers: readonly ProviderInfo[]
  phone: boolean
  selected: ProviderKind | null
  k8sContexts: number | null
  onSelect: (k: ProviderKind) => void
  onUse?: (p: ProviderInfo, id: string) => void
  using?: string | null
  grid?: boolean
}) {
  return (
    <div role="radiogroup" aria-label="Provider" className={cn(grid ? 'grid grid-cols-2 gap-3' : 'flex flex-col gap-2.5')}>
      {offered(providers, phone).map((p) => (
        <ProviderCard key={p.provider} p={p} phone={phone} selected={selected === p.provider} k8sContexts={k8sContexts} onSelect={onSelect} onUse={onUse} using={using === p.provider} />
      ))}
    </div>
  )
}

/** How the selected provider gets its credential: sign-in (with "Paste a token instead") or a pasted token. */
export type Connect = { mode: 'oauth' | 'token'; waiting: boolean; error: string | null; token: string }

export function ConnectPanel({ p, connect, onToken, onPasteInstead, onCancel }: { p: ProviderInfo; connect: Connect; onToken: (t: string) => void; onPasteInstead: () => void; onCancel: () => void }) {
  if (p.connected || !p.available || p.provider === 'kubernetes') return null
  if (connect.waiting)
    return (
      <Card className="flex items-center gap-2.5">
        <Loader2 className="size-4 animate-spin text-primary" />
        <div className="min-w-0 flex-1 text-sm">Waiting for {p.label}…</div>
        <Button size="sm" variant="outline" onClick={onCancel}>
          Cancel
        </Button>
      </Card>
    )
  return (
    <div className="flex flex-col gap-1.5">
      {connect.mode === 'token' ? (
        <>
          <Input type="password" autoComplete="off" placeholder={`${p.label} API token`} aria-label={`${p.label} API token`} value={connect.token} onChange={(e) => onToken(e.currentTarget.value)} />
          <Muted>Stored encrypted on this engine; used only to create and manage your nodes.</Muted>
        </>
      ) : (
        <button type="button" className="self-start text-xs text-fg-accent hover:underline" onClick={onPasteInstead}>
          Paste a token instead
        </button>
      )}
      {connect.error && <p className="text-xs text-danger">{connect.error}</p>}
    </div>
  )
}

/** The provider step's primary label: follows the selection and its connection. */
export function providerAction(p: ProviderInfo | undefined, connect: Connect): string {
  if (!p || p.connected || !p.available || p.provider === 'kubernetes') return 'Continue'
  return p.oauth && connect.mode === 'oauth' ? `Sign in with ${p.label}` : 'Continue'
}

export function PhoneProviderStep(props: {
  providers: readonly ProviderInfo[]
  selected: ProviderKind | null
  connect: Connect
  busy: boolean
  onSelect: (k: ProviderKind) => void
  onToken: (t: string) => void
  onPasteInstead: () => void
  onCancel: () => void
  onNext: () => void
}) {
  const p = props.providers.find((x) => x.provider === props.selected)
  const blocked = !p?.available || (!p.connected && props.connect.mode === 'token' && !props.connect.token.trim())
  return (
    <>
      <Muted>Step 1 of 3</Muted>
      <Title className="mt-0">Pick a provider</Title>
      <ProviderCards providers={props.providers} phone selected={props.selected} k8sContexts={null} onSelect={props.onSelect} />
      {p && <ConnectPanel p={p} connect={props.connect} onToken={props.onToken} onPasteInstead={props.onPasteInstead} onCancel={props.onCancel} />}
      <Muted>The bill is from your own provider. theuth never holds your card.</Muted>
      <div className="mt-auto pt-2">
        {!props.connect.waiting && (
          <Primary disabled={blocked} busy={props.busy} onClick={props.onNext}>
            {providerAction(p, props.connect)}
          </Primary>
        )}
      </div>
    </>
  )
}

// ---- ③ configure ----

export type ConfigureProps = {
  provider: ProviderInfo
  spec: NodeSpec
  quote: Quote | null
  nameError: string | null
  busy: boolean
  onName: (n: string) => void
  onRegion: (id: string) => void
  onMemory: (gb: number) => void
  onCreate: () => void
}

function RegionSelect({ regions, value, onChange, big }: { regions: readonly Region[]; value: string; onChange: (id: string) => void; big?: boolean }) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger aria-label="Region" className={cn('w-full', big && 'h-auto border-0 bg-transparent px-0 py-0.5 text-[15px] font-semibold shadow-none dark:bg-transparent')}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {regions.map((r) => (
          <SelectItem key={r.id} value={r.id}>
            {flag(r.country)} {r.label} <span className="text-fg-tertiary">({r.id})</span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

function SizeSeg({ provider, value, onChange }: { provider: ProviderInfo; value: number; onChange: (gb: number) => void }) {
  return (
    <div className="[&>div]:flex [&>div]:w-full [&_button]:flex-1 [&_button]:py-1.5 [&_button]:text-xs">
      <Seg label="Size" value={String(value)} onChange={(v) => onChange(Number(v))} items={provider.sizes.map((s) => ({ value: String(s.memory_gb), label: `${s.memory_gb} GB` }))} />
    </div>
  )
}

export const SIZE_HINT = '2 GB is the minimum. 4 GB for heavy browsing or local models.'

export function PhoneConfigureStep(c: ConfigureProps) {
  return (
    <>
      <Muted>Step 2 of 3</Muted>
      <Title className="mt-0">Your node</Title>
      <Card>
        <Muted>Name</Muted>
        <Input className="mt-1" aria-label="Name" aria-invalid={!!c.nameError || undefined} autoCapitalize="none" autoCorrect="off" spellCheck={false} value={c.spec.name} onChange={(e) => c.onName(e.currentTarget.value)} />
        {c.nameError && <p className="mt-1 text-xs text-danger">{c.nameError}</p>}
      </Card>
      <Card>
        <Muted>Region · nearest picked for you</Muted>
        <RegionSelect big regions={c.provider.regions} value={c.spec.region} onChange={c.onRegion} />
      </Card>
      <Card>
        <Muted className="mb-1.5">Size</Muted>
        <SizeSeg provider={c.provider} value={c.spec.memory_gb} onChange={c.onMemory} />
        <Muted className="mt-1.5">{SIZE_HINT}</Muted>
      </Card>
      <Card>
        <div className="text-[13px] font-semibold">{costLine(c.quote) ?? '—'}</div>
        <Muted>No open ports · updates itself</Muted>
      </Card>
      <div className="mt-auto pt-2">
        <Primary disabled={!!c.nameError} busy={c.busy} onClick={c.onCreate}>
          {createLabel(c.spec, c.quote)}
        </Primary>
      </div>
    </>
  )
}

/** The desktop dialog's footer: the cost on the left, Back and the action on the right. */
export function DesktopFooter({ left, onBack, action, disabled, busy, onAction }: { left: ReactNode; onBack?: () => void; action: string; disabled?: boolean; busy?: boolean; onAction: () => void }) {
  return (
    <div className="mt-auto flex items-center justify-between gap-4 border-t border-border pt-3">
      <span className="text-xs text-fg-secondary">{left}</span>
      <div className="flex shrink-0 gap-2">
        {onBack && (
          <Button variant="outline" onClick={onBack}>
            Back
          </Button>
        )}
        <Button disabled={disabled || busy} onClick={onAction}>
          {busy && <Loader2 className="animate-spin" />}
          {action}
        </Button>
      </div>
    </div>
  )
}

export function DesktopConfigureFields(c: ConfigureProps & { advancedOpen?: boolean; onBack?: () => void }) {
  return (
    <>
      <div className="grid grid-cols-3 gap-2.5">
        <Card>
          <Muted>Name</Muted>
          <Input className="mt-1 h-8" aria-label="Name" aria-invalid={!!c.nameError || undefined} spellCheck={false} value={c.spec.name} onChange={(e) => c.onName(e.currentTarget.value)} />
          {c.nameError && <p className="mt-1 text-xs text-danger">{c.nameError}</p>}
        </Card>
        <Card>
          <Muted className="mb-1">Region</Muted>
          <RegionSelect regions={c.provider.regions} value={c.spec.region} onChange={c.onRegion} />
        </Card>
        <Card>
          <Muted className="mb-1.5">Size</Muted>
          <SizeSeg provider={c.provider} value={c.spec.memory_gb} onChange={c.onMemory} />
          <Muted className="mt-1.5 text-[11px]">{SIZE_HINT}</Muted>
        </Card>
      </div>
      <Collapsible defaultOpen={c.advancedOpen}>
        <Card>
          <CollapsibleTrigger className="group flex w-full items-center gap-1.5 text-sm font-semibold">
            <ChevronDown className="size-3.5 -rotate-90 transition-transform group-data-[state=open]:rotate-0" />
            Advanced
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div className="mt-2 grid grid-cols-3 gap-2.5 text-[13px]">
              <div>
                <Muted>Disk</Muted>
                {c.spec.volume_gb ?? 10} GB · kept on delete
              </div>
              <div>
                <Muted>SSH access</Muted>
                Off (no inbound ports)
              </div>
              <div>
                <Muted>Updates</Muted>
                Follow stable releases
              </div>
            </div>
          </CollapsibleContent>
        </Card>
      </Collapsible>
      <DesktopFooter left={billedLine(c.quote, c.provider.label)} onBack={c.onBack} action={createLabel(c.spec, c.quote)} disabled={!!c.nameError} busy={c.busy} onAction={c.onCreate} />
    </>
  )
}

// ---- ③ Kubernetes variant ----

export const FLANNEL_WARNING = "doesn't enforce NetworkPolicies. This would have no effect here."

export function K8sConfigure({ info, k8s, memory_gb, busy, onChange, onDeploy, onBack, loading }: { info: K8sInfo | null; k8s: K8sOptions; memory_gb: number; busy: boolean; onChange: (k: K8sOptions) => void; onDeploy: () => void; onBack?: () => void; loading?: boolean }) {
  const namespaces = [...new Set([...(info?.namespaces ?? []), ...(k8s.namespace ? [k8s.namespace] : [])])]
  const creating = !!k8s.namespace && !(info?.namespaces ?? []).includes(k8s.namespace)
  const set = (patch: Partial<K8sOptions>) => onChange({ ...k8s, ...patch })
  return (
    <>
      <div className="grid grid-cols-3 gap-2.5">
        <Card>
          <Muted className="mb-1">Context</Muted>
          <Select value={k8s.context ?? ''} onValueChange={(context) => set({ context, storage_class: null })}>
            <SelectTrigger aria-label="Context" className="w-full">
              <SelectValue placeholder={loading ? 'Loading…' : 'No context'} />
            </SelectTrigger>
            <SelectContent>
              {(info?.contexts ?? []).map((c) => (
                <SelectItem key={c} value={c}>
                  {c}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Card>
        <Card>
          <Muted className="mb-1 flex items-center gap-1.5">Namespace {creating && <Chip>create</Chip>}</Muted>
          <Select value={k8s.namespace ?? ''} onValueChange={(namespace) => set({ namespace, create_namespace: !(info?.namespaces ?? []).includes(namespace) })}>
            <SelectTrigger aria-label="Namespace" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {namespaces.map((n) => (
                <SelectItem key={n} value={n}>
                  {n}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {creating && <Muted className="mt-1 text-[11px]">Created on deploy</Muted>}
        </Card>
        <Card>
          <Muted className="mb-1">Storage class</Muted>
          <Select value={k8s.storage_class ?? ''} onValueChange={(storage_class) => set({ storage_class })}>
            <SelectTrigger aria-label="Storage class" className="w-full">
              <SelectValue placeholder="None" />
            </SelectTrigger>
            <SelectContent>
              {(info?.storage_classes ?? []).map((s) => (
                <SelectItem key={s.name} value={s.name}>
                  {s.name}
                  {s.default ? ' (default)' : ''}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {info?.no_default_class && <Muted className="mt-1 text-[11px] text-warning">No default class in this cluster</Muted>}
        </Card>
      </div>
      <Card>
        <label className="flex items-start gap-2 text-[13px]">
          <input type="checkbox" className="mt-0.5 size-4 accent-[var(--primary)]" checked={!!k8s.deny_cluster_network} onChange={(e) => set({ deny_cluster_network: e.currentTarget.checked })} />
          <span>
            <b>Block cluster network</b> — the node can reach the internet and your mesh, not other pods.
          </span>
        </label>
        {info?.enforces_network_policy === false && (
          <Muted className="mt-1 pl-6 text-warning">
            ⚠ This cluster runs {info.cni ?? 'a CNI that'}, which {FLANNEL_WARNING}
          </Muted>
        )}
      </Card>
      <DesktopFooter left={k8sLine(memory_gb)} onBack={onBack} action={`Deploy to ${k8s.context ?? 'cluster'}`} disabled={!k8s.context} busy={busy} onAction={onDeploy} />
    </>
  )
}

// ---- ④ progress ----

function StageDot({ status }: { status: StageRow['status'] }) {
  if (status === 'done')
    return (
      <span className="grid size-4 shrink-0 place-items-center rounded-full bg-success text-white">
        <Check className="size-2.5" strokeWidth={3.5} />
      </span>
    )
  if (status === 'active') return <span className="size-4 shrink-0 animate-spin rounded-full border-2 border-primary border-t-transparent" />
  if (status === 'failed') return <span className="grid size-4 shrink-0 place-items-center rounded-full bg-danger text-[10px] font-bold text-white">!</span>
  return <span className="size-4 shrink-0 rounded-full border-2 border-border" />
}

export function StageList({ rows }: { rows: readonly StageRow[] }) {
  return (
    <ol className="flex flex-col">
      {rows.map((r) => (
        <li key={r.stage} data-status={r.status} className={cn('flex items-center gap-2 py-1 text-[13px]', r.status === 'active' && 'font-semibold', r.status === 'waiting' && 'text-fg-tertiary', r.status === 'failed' && 'font-semibold text-danger')}>
          <StageDot status={r.status} />
          {r.label}
        </li>
      ))}
    </ol>
  )
}

export function ProgressView({ name, rows, pct, eta, error, retrying, phone, onRetry }: { name: string; rows: readonly StageRow[]; pct: number; eta: number | null; error: string | null; retrying: boolean; phone: boolean; onRetry: () => void }) {
  const failed = rows.some((r) => r.status === 'failed')
  const left = etaText(eta)
  return (
    <>
      {phone && <Muted>Step 3 of 3</Muted>}
      <Title className={phone ? 'mt-0' : ''}>{failed ? `${name} could not start` : `Starting ${name}`}</Title>
      <div className="h-1.5 overflow-hidden rounded-full bg-layer-2" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <i className={cn('block h-full transition-[width]', failed ? 'bg-danger' : 'bg-primary')} style={{ width: `${pct}%` }} />
      </div>
      {!failed && <Muted>{left ? `${left} · you can leave this screen` : 'You can leave this screen'}</Muted>}
      <Card>
        <StageList rows={rows} />
      </Card>
      {failed ? (
        <div className="flex flex-col gap-2">
          {error && <p className="rounded-lg border border-danger-border/50 bg-danger-bg px-3 py-2 text-xs text-danger">{error}</p>}
          <Muted>Destroying removes everything this attempt made at the provider, then you can try again.</Muted>
          <div className={cn(phone ? 'mt-auto' : 'self-start')}>
            <Button variant="destructive" className={cn(phone && 'h-11 w-full')} disabled={retrying} onClick={onRetry}>
              {retrying && <Loader2 className="animate-spin" />}
              Destroy and retry
            </Button>
          </div>
        </div>
      ) : (
        <Note>We'll notify you when it's ready.</Note>
      )}
    </>
  )
}

// ---- ⑤ done ----

export function doneText(entry: WizardEntry, name: string): string {
  return entry === 'first-run'
    ? `Your phone and ${name} are now one mesh. Chats run on the node, so they keep going when you lock your phone.`
    : `${name} joined your mesh. Chats and routines can run on it, so they keep going when this device sleeps.`
}

export function DoneView({ name, entry, onModel, onLaptop }: { name: string; entry: WizardEntry; onModel: () => void; onLaptop: () => void }) {
  return (
    <>
      <div className="mt-5 text-center text-[40px] leading-none">☁︎</div>
      <Title className="text-center">{name} is ready</Title>
      <Muted className="text-center text-[13px]">{doneText(entry, name)}</Muted>
      <Card>
        <div className="text-sm font-semibold">Next: connect a model</div>
        <Muted>Sign in with Claude, ChatGPT or Gemini. The key lives on your node, encrypted.</Muted>
      </Card>
      <div className="mt-auto flex flex-col gap-2 pt-2">
        <Primary onClick={onModel}>Connect a model</Primary>
        <Button variant="outline" className="h-11 w-full rounded-[10px] text-[15px] font-semibold" onClick={onLaptop}>
          Add my laptop too
        </Button>
      </div>
    </>
  )
}

// ---- desktop rail ----

export type RailStep = 'provider' | 'configure' | 'create'
const RAIL: [RailStep, string][] = [
  ['provider', 'Provider'],
  ['configure', 'Configure'],
  ['create', 'Create'],
]

export function Rail({ at, title }: { at: RailStep; title: ReactNode }) {
  const i = RAIL.findIndex(([s]) => s === at)
  return (
    <aside className="flex flex-col border-r border-border bg-card p-4">
      {title}
      <ol className="mt-3 flex flex-col gap-0.5 text-[13px]">
        {RAIL.map(([s, label], j) => (
          <li key={s} aria-current={j === i ? 'step' : undefined} className={cn('rounded-lg px-2 py-1.5 text-fg-tertiary', j === i && 'bg-layer-2 font-semibold text-foreground', j < i && 'text-foreground')}>
            {j < i && <span className="text-success">✓ </span>}
            {label}
          </li>
        ))}
      </ol>
      <Note className="mt-5">Why a cloud node? It runs when this Mac sleeps, handles schedules overnight, and gives your phone a full-power engine.</Note>
    </aside>
  )
}
