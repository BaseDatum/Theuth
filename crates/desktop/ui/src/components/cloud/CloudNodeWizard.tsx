// 224: create a hosted, always-on theuth node on the user's own DigitalOcean / Fly.io / Kubernetes account.
// One wizard, three entries: the phone's first run (① choose … ⑤ done), the phone's "Add a cloud node" (no ①), and the
// desktop dialog (step rail + provider grid + 3-column configure). The screens are `steps.tsx`; this owns the state,
// the RPCs and the OAuth wait. The press on "Create node · $13/mo" is the approval of the cost.

import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ChevronLeft, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { useApi } from '@/lib/rpc'
import { openExternal } from '@/lib/platform'
import { useConnection } from '@/store/connection'
import { checkName, defaultMemory, defaultName, desktopBlocked, fromPrice, localQuote, offered, progressPct, sizesOf, stageRows, userOf } from './format'
import { cloudKeys, useCloudActions, useCloudEvents, useCloudList, useCloudNode, useCloudProgress, useCloudProviders, useCloudSizes, useCredentialChanged, useK8sInfo, useVaultSupport } from './queries'
import { nearestRegion } from './region'
import {
  ChooseStep,
  type Choice,
  type Connect,
  ConnectPanel,
  DesktopConfigureFields,
  DesktopFooter,
  DoneView,
  K8sConfigure,
  PhoneConfigureStep,
  PhoneProviderStep,
  ProgressView,
  ProviderCards,
  Rail,
  type RailStep,
  type WizardEntry,
  providerAction,
} from './steps'
import { DEFAULT_VOLUME_GB, type CloudNode, type K8sOptions, type NodeSpec, type ProviderInfo, type ProviderKind, type SizeList } from './types'

export type { WizardEntry } from './steps'
export type WizardStep = 'choose' | 'provider' | 'configure' | 'progress' | 'done'

/** Where the wizard opens (the fixture opens any screen; a real entry passes nothing). */
export type WizardInitial = {
  step?: WizardStep
  spec?: Partial<Omit<NodeSpec, 'k8s'>> & { k8s?: K8sOptions | null }
  /** The node a `progress` / `done` screen follows. */
  node?: string
  connect?: Partial<Connect>
  advancedOpen?: boolean
}

const DEFAULT_NAMESPACE = 'theuth-nodes'

export function CloudNodeWizard({ entry, onDone, onClose, initial }: { entry: WizardEntry; onDone?: (node: CloudNode | null) => void; onClose: () => void; initial?: WizardInitial }) {
  const desktop = entry === 'desktop-dialog'
  const phone = !desktop
  useCloudEvents()
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const info = useConnection((s) => s.info)
  const providersQ = useCloudProviders()
  const list = useCloudList()
  const actions = useCloudActions()
  const providers = useMemo(() => offered(providersQ.data ?? [], phone), [providersQ.data, phone])

  const [step, setStep] = useState<WizardStep>(initial?.step ?? (entry === 'first-run' ? 'choose' : 'provider'))
  const [choice, setChoice] = useState<Choice>('cloud')
  const [picked, setPicked] = useState<ProviderKind | null>(initial?.spec?.provider ?? null)
  const [connect, setConnect] = useState<Connect>({ mode: 'oauth', waiting: false, error: null, token: '', ...initial?.connect })
  const [name, setName] = useState<string | null>(initial?.spec?.name ?? null)
  const [regions, setRegions] = useState<Partial<Record<ProviderKind, string>>>(initial?.spec?.provider && initial.spec.region ? { [initial.spec.provider]: initial.spec.region } : {})
  // The size the user picked; until then it follows the desktop choice (4 GB with it, 2 GB without).
  const [memoryPick, setMemory] = useState<number | null>(initial?.spec?.memory_gb ?? null)
  const [desktopPick, setDesktop] = useState(initial?.spec?.desktop ?? true)
  // The sealed vault's master password: shown only where the engine has the sealed vault, and kept here, out of
  // `NodeSpec`, until the merge with feat/sealed-vault wires it into the create call.
  const vaultOn = useVaultSupport()
  const [vaultPassword, setVaultPassword] = useState('')
  const vault = vaultOn ? { password: vaultPassword, onPassword: setVaultPassword } : null
  const [k8s, setK8s] = useState<K8sOptions | null>(initial?.spec?.k8s ?? null)
  const [created, setCreated] = useState<string | null>(initial?.node ?? null)

  // The default pick: a connected provider that can create, else the first that can.
  const provider: ProviderInfo | undefined = providers.find((p) => p.provider === picked) ?? providers.find((p) => p.available && p.connected && p.provider !== 'kubernetes') ?? providers.find((p) => p.available)
  const kind = provider?.provider ?? null
  const isK8s = kind === 'kubernetes'

  const taken = useMemo(() => (list.data?.nodes ?? []).map((n) => n.name), [list.data])
  const nodeName = name ?? defaultName(userOf(info?.data_dir), taken)
  const region = (kind && regions[kind]) ?? (provider ? (nearestRegion(provider.regions)?.id ?? '') : '')

  const k8sInfo = useK8sInfo(k8s?.context ?? null, desktop && providers.some((p) => p.provider === 'kubernetes'))
  // Kubernetes defaults from the cluster: current context, `theuth-nodes`, the default storage class.
  const k8sOpts: K8sOptions = {
    context: k8s?.context ?? k8sInfo.data?.current_context ?? k8sInfo.data?.contexts[0] ?? null,
    namespace: k8s?.namespace ?? DEFAULT_NAMESPACE,
    create_namespace: k8s?.create_namespace ?? !(k8sInfo.data?.namespaces ?? []).includes(k8s?.namespace ?? DEFAULT_NAMESPACE),
    storage_class: k8s?.storage_class ?? k8sInfo.data?.storage_classes.find((s) => s.default)?.name ?? k8sInfo.data?.storage_classes[0]?.name ?? null,
    deny_cluster_network: k8s?.deny_cluster_network ?? false,
  }

  const desktopOn = !!kind && desktopPick && !desktopBlocked(kind)
  const memory = memoryPick ?? defaultMemory(desktopOn)
  const spec: NodeSpec | null = kind ? { provider: kind, name: nodeName, region: isK8s ? '' : region, memory_gb: memory, volume_gb: DEFAULT_VOLUME_GB, k8s: isK8s ? k8sOpts : null, desktop: desktopOn } : null
  // Live sizes and prices for the chosen provider and region (needs its credential; Kubernetes needs none).
  const sizesQ = useCloudSizes(kind, isK8s ? null : region || null, !!provider && (provider.connected || isK8s))
  const sizes = sizesOf(sizesQ.data) ?? (sizesQ.isError ? provider?.sizes.map((s) => ({ ...s, monthly_usd: null })) : undefined)
  const quote = kind && sizes ? localQuote(kind, sizes, provider?.volume_usd_per_gb ?? null, memory) : null
  const nameError = checkName(nodeName) ?? (taken.includes(nodeName) && nodeName !== created ? `${nodeName} is taken` : null)

  const node = useCloudNode(created)
  const progress = useCloudProgress(created)
  const rows = stageRows({ node, progress, version: node?.version ?? info?.version ?? null })
  useEffect(() => {
    if (step === 'progress' && node?.state === 'ready') setStep('done')
  }, [step, node?.state])

  // OAuth: the browser half ends in `CloudCredentialChanged`; a connected provider moves on to configure.
  const onCredential = useCallback(
    (e: { connected: boolean; error: string | null }) => {
      setConnect((c) => ({ ...c, waiting: false, error: e.connected ? null : (e.error ?? 'The sign-in did not finish') }))
      if (e.connected) {
        void qc.invalidateQueries({ queryKey: cloudKeys.providers })
        setStep('configure')
      }
    },
    [qc],
  )
  useCredentialChanged(connect.waiting ? kind : null, onCredential)

  const select = (k: ProviderKind) => {
    if (connect.waiting && kind) void api.cloud_oauth_cancel(kind).catch(() => undefined)
    setPicked(k)
    setConnect({ mode: 'oauth', waiting: false, error: null, token: '' })
  }

  const signIn = async () => {
    if (!kind) return
    setConnect((c) => ({ ...c, waiting: true, error: null }))
    try {
      const { url } = await api.cloud_oauth_start(kind)
      openExternal(url)
    } catch (e) {
      setConnect((c) => ({ ...c, waiting: false, error: errorText(e) }))
    }
  }
  const cancelSignIn = () => {
    if (kind) void api.cloud_oauth_cancel(kind).catch(() => undefined)
    setConnect((c) => ({ ...c, waiting: false }))
  }

  /** The provider step's primary: continue when connected, else sign in or save the pasted token. */
  const next = () => {
    if (!provider) return
    if (provider.connected || provider.provider === 'kubernetes') return setStep('configure')
    if (provider.oauth && connect.mode === 'oauth') return void signIn()
    actions.setToken.mutate(
      { provider: provider.provider, token: connect.token.trim() },
      {
        onSuccess: () => {
          setConnect((c) => ({ ...c, token: '', error: null }))
          setStep('configure')
        },
        onError: (e) => setConnect((c) => ({ ...c, error: errorText(e) })),
      },
    )
  }

  const create = () => {
    if (!spec || (nameError && !isK8s)) return
    actions.create.mutate(spec, {
      onSuccess: (n) => {
        setCreated(n.name)
        setStep('progress')
      },
    })
  }

  const retry = () => {
    if (!created) return
    actions.destroy.mutate(
      { name: created, keep: false },
      {
        onSuccess: () => {
          qc.removeQueries({ queryKey: cloudKeys.progress(created) })
          setCreated(null)
          setStep('configure')
        },
      },
    )
  }

  const finish = (to: 'providers' | 'machines') => {
    onDone?.(node)
    void navigate({ to: '/settings/$tab', params: { tab: to } })
  }

  const back = () => {
    if (step === 'configure') setStep('provider')
    else if (step === 'provider' && entry === 'first-run') setStep('choose')
    else onClose()
  }

  const loading = providersQ.isPending ? <p className="text-sm text-fg-tertiary">Loading providers…</p> : providersQ.isError ? <p className="text-sm text-danger">{providersQ.error.message}</p> : null

  const configure = provider && spec && (
    <>
      {isK8s ? (
        <K8sConfigure info={k8sInfo.data ?? null} sizes={sizes} loading={k8sInfo.isPending} k8s={k8sOpts} memory_gb={memory} busy={actions.create.isPending} onChange={setK8s} onMemory={setMemory} onDeploy={create} onBack={back} />
      ) : phone ? (
        <PhoneConfigureStep
          provider={provider}
          spec={spec}
          sizes={sizes}
          quote={quote}
          nameError={nameError}
          busy={actions.create.isPending}
          onName={setName}
          onRegion={(id) => setRegions((r) => ({ ...r, [provider.provider]: id }))}
          onMemory={setMemory}
          onDesktop={setDesktop}
          vault={vault}
          onCreate={create}
        />
      ) : (
        <DesktopConfigureFields
          provider={provider}
          spec={spec}
          sizes={sizes}
          quote={quote}
          nameError={nameError}
          busy={actions.create.isPending}
          advancedOpen={initial?.advancedOpen}
          onBack={back}
          onName={setName}
          onRegion={(id) => setRegions((r) => ({ ...r, [provider.provider]: id }))}
          onMemory={setMemory}
          onDesktop={setDesktop}
          vault={vault}
          onCreate={create}
        />
      )}
    </>
  )

  const progressView = created && <ProgressView name={created} rows={rows} pct={progressPct(rows, progress)} eta={progress?.eta_secs ?? null} error={node?.error ?? null} retrying={actions.destroy.isPending} phone={phone} onRetry={retry} />
  const doneView = <DoneView name={created ?? nodeName} entry={entry} onModel={() => finish('providers')} onLaptop={() => finish('machines')} />

  if (phone) {
    const first: WizardStep = entry === 'first-run' ? 'choose' : 'provider'
    const canBack = step === 'configure' || (step === 'provider' && first === 'choose')
    let body
    if (step === 'choose')
      body = (
        <ChooseStep
          fromPrice={fromPrice(providersQ.data ?? [], true, Object.fromEntries(qc.getQueriesData<SizeList>({ queryKey: ['cloud', 'sizes'] }).flatMap(([key, data]) => (data ? [[key[2], data]] : []))))}
          choice={choice}
          onChoice={setChoice}
          onContinue={() => (choice === 'cloud' ? setStep('provider') : onDone ? onDone(null) : onClose())}
        />
      )
    else if (step === 'provider')
      body = loading ?? (
        <PhoneProviderStep
          providers={providers}
          selected={kind}
          connect={connect}
          busy={actions.setToken.isPending}
          onSelect={select}
          onToken={(token) => setConnect((c) => ({ ...c, token }))}
          onPasteInstead={() => setConnect((c) => ({ ...c, mode: 'token', error: null }))}
          onCancel={cancelSignIn}
          onNext={next}
        />
      )
    else if (step === 'configure') body = loading ?? configure
    else if (step === 'progress') body = progressView
    else body = doneView
    return (
      <div data-cloud-wizard={entry} className="flex min-h-full flex-col bg-background px-4 pt-[calc(var(--safe-top)+8px)] pb-[calc(var(--safe-bottom)+16px)] text-foreground">
        <header className="flex h-9 shrink-0 items-center justify-between">
          {canBack ? (
            <Button variant="ghost" size="icon-sm" aria-label="Back" className="-ml-2" onClick={back}>
              <ChevronLeft className="size-5" />
            </Button>
          ) : (
            <span />
          )}
          {step !== 'done' && (
            <Button variant="ghost" size="icon-sm" aria-label="Close" className="-mr-2" onClick={onClose}>
              <X className="size-5" />
            </Button>
          )}
        </header>
        <div className="flex flex-1 flex-col gap-2.5">{body}</div>
      </div>
    )
  }

  // Desktop: the rail, then one pane per step.
  const rail: RailStep = step === 'progress' || step === 'done' ? 'create' : step === 'configure' ? 'configure' : 'provider'
  let pane
  if (step === 'provider' || step === 'choose')
    pane = loading ?? (
      <>
        <ProviderCards providers={providers} phone={false} grid selected={kind} k8sContexts={k8sInfo.data?.contexts.length ?? null} onSelect={select} onUse={(p, id) => actions.importCredential.mutate({ provider: p.provider, id })} using={actions.importCredential.isPending ? (actions.importCredential.variables?.provider ?? null) : null} />
        {provider && (
          <ConnectPanel
            p={provider}
            connect={connect}
            onToken={(token) => setConnect((c) => ({ ...c, token }))}
            onPasteInstead={() => setConnect((c) => ({ ...c, mode: 'token', error: null }))}
            onCancel={cancelSignIn}
          />
        )}
        <p className="text-xs text-fg-secondary">The bill is from your own provider. theuth never holds your card.</p>
        {!connect.waiting && (
          <DesktopFooter
            left={provider?.discovered.length && !provider.connected ? `Found in ${provider.discovered[0]?.path}` : null}
            onBack={onClose}
            action={providerAction(provider, connect)}
            disabled={!provider?.available || (!provider.connected && !isK8s && connect.mode === 'token' && !connect.token.trim())}
            busy={actions.setToken.isPending}
            onAction={next}
          />
        )}
      </>
    )
  else if (step === 'configure')
    pane = loading ?? (
      <>
        <ProviderCards providers={providers.filter((p) => p.available && (p.connected || p.provider === kind))} phone={false} grid selected={kind} k8sContexts={k8sInfo.data?.contexts.length ?? null} onSelect={select} />
        {configure}
      </>
    )
  else if (step === 'progress')
    pane = (
      <>
        {progressView}
        <DesktopFooter left="Closing this keeps the node starting; Settings › Machines shows it." action="Close" onAction={onClose} />
      </>
    )
  else pane = <div className="mx-auto flex w-full max-w-sm flex-1 flex-col gap-2.5">{doneView}</div>

  // The dialog is the wizard's own: mounting the desktop entry opens it, closing it calls `onClose`.
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] gap-0 overflow-y-auto p-0 sm:max-w-[980px]">
        <DialogDescription className="sr-only">Create an always-on theuth node on your own cloud account.</DialogDescription>
        <div data-cloud-wizard={entry} className="grid min-h-[460px] grid-cols-[200px_1fr] text-foreground">
          <Rail at={rail} title={<DialogTitle className="text-sm font-semibold">New cloud node</DialogTitle>} />
          <main className="flex min-w-0 flex-col gap-3 px-6 pt-12 pb-5">{pane}</main>
        </div>
      </DialogContent>
    </Dialog>
  )
}
