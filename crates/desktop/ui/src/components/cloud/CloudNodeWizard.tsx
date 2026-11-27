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
import { isPeerBootstrapping, usePeerSync } from '@/lib/peer-sync'
import { isIos, openExternal } from '@/lib/platform'
import { closeAuthBrowser, onAuthBrowserDismissed, openAuthBrowser } from '@/lib/tauri'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { usePhoneLink } from './phoneLink'
import { checkName, defaultMemory, defaultName, desktopBlocked, fromPrice, localQuote, offered, progressPct, sizesOf, stageRows, userOf } from './format'
import { cloudKeys, useCloudActions, useCloudEvents, useCloudList, useCloudNode, useCloudProgress, useCloudProviders, useCloudSizes, useCredentialChanged, useK8sInfo, useVaultSupport } from './queries'
import { nearestRegion } from './region'
import { SshConfigure, type SshInputMode, parseSshPort, suggestSshNodeName } from './SshConfigure'
import { useSshHosts, useSshProbe } from './sshApi'
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
import { DEFAULT_VOLUME_GB, type CloudNode, type K8sOptions, type NodeSpec, type ProviderInfo, type ProviderKind, type SizeList, type SshHostEntry, type SshOptions, type SshProbe } from './types'

export type { WizardEntry } from './steps'
export type WizardStep = 'choose' | 'provider' | 'configure' | 'progress' | 'done'

/** Where the wizard opens (the fixture opens any screen; a real entry passes nothing). */
export type WizardInitial = {
  step?: WizardStep
  spec?: Partial<Omit<NodeSpec, 'k8s' | 'ssh'>> & { k8s?: K8sOptions | null; ssh?: SshOptions | null }
  /** The node a `progress` / `done` screen follows. */
  node?: string
  connect?: Partial<Connect>
  advancedOpen?: boolean
  /** The vault field as typed (the fixture's screenshots). */
  vault?: { password: string; confirm: string }
}

const DEFAULT_NAMESPACE = 'theuth-nodes'
/** 224 §16.12: a phone-only create keeps going while the app is away; the engine resumes it on return. */
export const LEAVE_HINT = "You can leave theuth — we'll finish when you're back."
const VAULT_LATER = 'Vault protection arrives with the next theuth release on this node; set a master password in Settings › Security then.'
const VAULT_FAILED = 'The master password was not set on the node; set it in Settings › Security › Vault.'

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
  const { byPeer } = usePeerSync()
  const actions = useCloudActions()
  const providers = useMemo(() => offered(providersQ.data ?? [], phone), [providersQ.data, phone])

  const [step, setStep] = useState<WizardStep>(initial?.step ?? (entry === 'first-run' ? 'choose' : 'provider'))
  const [choice, setChoice] = useState<Choice>('cloud')
  const [picked, setPicked] = useState<ProviderKind | null>(initial?.spec?.provider ?? null)
  const [connect, setConnect] = useState<Connect>({ mode: 'oauth', waiting: false, error: null, token: '', ...initial?.connect })
  const [name, setName] = useState<string | null>(initial?.spec?.name ?? null)
  const [nameTouched, setNameTouched] = useState(initial?.spec?.name != null)
  const [regions, setRegions] = useState<Partial<Record<ProviderKind, string>>>(initial?.spec?.provider && initial.spec.region ? { [initial.spec.provider]: initial.spec.region } : {})
  // The size the user picked; until then it follows the desktop choice (4 GB with it or on Fly.io, 2 GB otherwise).
  const [memoryPick, setMemory] = useState<number | null>(initial?.spec?.memory_gb ?? null)
  const [desktopPick, setDesktop] = useState(initial?.spec?.desktop ?? true)
  // The node's master password (design 225), only where the engine has the sealed vault and takes it in
  // `cloud_create`. Kept out of `NodeSpec` (the engine never stores it) and cleared once the create call returns.
  const vaultOn = useVaultSupport()
  const [vaultPassword, setVaultPassword] = useState(initial?.vault?.password ?? '')
  const [vaultConfirm, setVaultConfirm] = useState(initial?.vault?.confirm ?? '')
  const vault = vaultOn ? { password: vaultPassword, confirm: vaultConfirm, onPassword: setVaultPassword, onConfirm: setVaultConfirm } : null
  const [k8s, setK8s] = useState<K8sOptions | null>(initial?.spec?.k8s ?? null)
  const [sshMode, setSshMode] = useState<SshInputMode>(initial?.spec?.ssh ? 'manual' : 'config')
  const [ssh, setSsh] = useState<SshOptions>(initial?.spec?.ssh ?? { host: '', user: null, port: null, identity: null, name: null })
  const [sshPortText, setSshPortText] = useState<string>(initial?.spec?.ssh?.port != null ? String(initial.spec.ssh.port) : '')
  const [sshProbeResult, setSshProbeResult] = useState<SshProbe | null>(null)
  const [sshProbeError, setSshProbeError] = useState<string | null>(null)
  const [created, setCreated] = useState<string | null>(initial?.node ?? null)

  // The default pick: a connected provider that can create, else the first that can.
  const provider: ProviderInfo | undefined = providers.find((p) => p.provider === picked) ?? providers.find((p) => p.available && p.connected && p.provider !== 'kubernetes' && p.provider !== 'ssh') ?? providers.find((p) => p.available)
  const kind = provider?.provider ?? null
  const isK8s = kind === 'kubernetes'
  const isSsh = kind === 'ssh'

  const taken = useMemo(() => (list.data?.nodes ?? []).map((n) => n.name), [list.data])
  const nodeName = name ?? defaultName(userOf(info?.data_dir), taken)
  const region = (kind && regions[kind]) ?? (provider ? (nearestRegion(provider.regions)?.id ?? '') : '')

  const k8sInfo = useK8sInfo(k8s?.context ?? null, desktop && providers.some((p) => p.provider === 'kubernetes'))
  const sshHostsQ = useSshHosts(desktop && providers.some((p) => p.provider === 'ssh'))
  const sshProbeM = useSshProbe()
  // Kubernetes defaults from the cluster: current context, `theuth-nodes`, the default storage class.
  const k8sOpts: K8sOptions = {
    context: k8s?.context ?? k8sInfo.data?.current_context ?? k8sInfo.data?.contexts[0] ?? null,
    namespace: k8s?.namespace ?? DEFAULT_NAMESPACE,
    create_namespace: k8s?.create_namespace ?? !(k8sInfo.data?.namespaces ?? []).includes(k8s?.namespace ?? DEFAULT_NAMESPACE),
    storage_class: k8s?.storage_class ?? k8sInfo.data?.storage_classes.find((s) => s.default)?.name ?? k8sInfo.data?.storage_classes[0]?.name ?? null,
    deny_cluster_network: k8s?.deny_cluster_network ?? false,
  }

  const effectiveSshMode: SshInputMode = (sshHostsQ.data?.hosts.length ?? 0) > 0 ? sshMode : 'manual'
  const parsedSshPort = parseSshPort(sshPortText)
  const sshPortError = effectiveSshMode === 'manual' ? parsedSshPort.error : null
  const sshOpts: SshOptions = {
    host: ssh.host.trim(),
    user: ssh.user?.trim() || null,
    port: effectiveSshMode === 'manual' ? parsedSshPort.port : (ssh.port ?? null),
    identity: ssh.identity?.trim() || null,
    name: null,
  }

  const desktopOn = !!kind && desktopPick && !desktopBlocked(kind)
  const memory = memoryPick ?? defaultMemory(desktopOn, kind)
  const spec: NodeSpec | null = kind
    ? {
        provider: kind,
        name: nodeName,
        region: isK8s || isSsh ? '' : region,
        memory_gb: isSsh ? 0 : memory,
        volume_gb: isSsh ? null : DEFAULT_VOLUME_GB,
        k8s: isK8s ? k8sOpts : null,
        ssh: isSsh ? { ...sshOpts, name: nodeName } : null,
        desktop: desktopOn,
      }
    : null
  // Live sizes and prices for the chosen provider and region (needs its credential; Kubernetes needs none; SSH has no sizes).
  const sizesQ = useCloudSizes(kind, isK8s ? null : region || null, !!provider && !isSsh && (provider.connected || isK8s))
  const sizes = sizesOf(sizesQ.data) ?? (sizesQ.isError ? provider?.sizes.map((s) => ({ ...s, monthly_usd: null })) : undefined)
  const quote = kind && !isSsh && sizes ? localQuote(kind, sizes, provider?.volume_usd_per_gb ?? null, memory) : null
  const nameError = checkName(nodeName) ?? (taken.includes(nodeName) && nodeName !== created ? `${nodeName} is taken` : null)

  const node = useCloudNode(created)
  // The connected engine is this iPhone/iPad's own: not a mesh member, so its nodes form their own mesh (224 §16.12),
  // and the system browser does not open from the app (the in-app sheet, `ios_auth`, does).
  const active = useEngines((s) => s.active)
  const onPhoneEngine = isIos() && active === LOCAL
  // Phone-only: the node is paired with this device over the relay once it is ready, and the master password goes to
  // the node over that link (never to this engine), so it is kept here until then.
  const phoneLink = usePhoneLink(api, node, vaultPassword)
  useEffect(() => {
    if (phoneLink.state.phase !== 'paired') return
    setVaultPassword('')
    setVaultConfirm('')
  }, [phoneLink.state.phase])
  const progress = useCloudProgress(created)
  const rows = stageRows({ node, progress, version: node?.version ?? info?.version ?? null })
  useEffect(() => {
    if (step === 'progress' && node?.state === 'ready') setStep('done')
  }, [step, node?.state])

  // OAuth: the browser half ends in `CloudCredentialChanged`; a connected provider moves on to configure.
  const onCredential = useCallback(
    (e: { connected: boolean; error: string | null }) => {
      setConnect((c) => ({ ...c, waiting: false, error: e.connected ? null : (e.error ?? 'The sign-in did not finish') }))
      if (onPhoneEngine) void closeAuthBrowser()
      if (e.connected) {
        void qc.invalidateQueries({ queryKey: cloudKeys.providers })
        setStep('configure')
      }
    },
    [qc, onPhoneEngine],
  )
  useCredentialChanged(connect.waiting ? kind : null, onCredential)

  const handleName = (v: string) => {
    setNameTouched(true)
    setName(v)
  }

  const resetSshProbe = () => {
    setSshProbeResult(null)
    setSshProbeError(null)
  }

  const updateSsh = (next: SshOptions) => {
    setSsh(next)
    resetSshProbe()
  }

  const pickSshHost = (entry: SshHostEntry) => {
    setSsh({ host: entry.alias, user: null, port: null, identity: null, name: null })
    setSshPortText('')
    if (!nameTouched) setName(suggestSshNodeName(entry.alias, taken))
    resetSshProbe()
  }

  const updateSshPort = (text: string) => {
    setSshPortText(text)
    const parsed = parseSshPort(text)
    setSsh((prev) => ({ ...prev, port: parsed.port }))
    resetSshProbe()
  }

  const checkSshServer = () => {
    if (!sshOpts.host || sshPortError) return
    setSshProbeResult(null)
    setSshProbeError(null)
    sshProbeM.mutate(sshOpts, {
      onSuccess: (res) => setSshProbeResult(res),
      onError: (e) => setSshProbeError(errorText(e)),
    })
  }

  const select = (k: ProviderKind) => {
    if (connect.waiting && kind) void api.cloud_oauth_cancel(kind).catch(() => undefined)
    const nextProvider = providers.find((p) => p.provider === k)
    setPicked(k)
    setConnect({ mode: nextProvider?.oauth ? 'oauth' : 'token', waiting: false, error: null, token: '' })
  }

  const signIn = async () => {
    if (!kind) return
    setConnect((c) => ({ ...c, waiting: true, error: null }))
    try {
      const { url } = await api.cloud_oauth_start(kind)
      if (onPhoneEngine) void openAuthBrowser(url, false)
      else openExternal(url)
    } catch (e) {
      setConnect((c) => ({ ...c, waiting: false, error: errorText(e) }))
    }
  }
  const cancelSignIn = () => {
    if (kind) void api.cloud_oauth_cancel(kind).catch(() => undefined)
    setConnect((c) => ({ ...c, waiting: false }))
  }
  // The user closed the in-app sign-in sheet: stop waiting for the relay.
  useEffect(() => {
    if (!onPhoneEngine || !connect.waiting || !kind) return
    const off = onAuthBrowserDismissed(() => {
      void api.cloud_oauth_cancel(kind).catch(() => undefined)
      setConnect((c) => ({ ...c, waiting: false }))
    })
    return () => void off.then((f) => f())
  }, [onPhoneEngine, connect.waiting, kind, api])

  /** The provider step's primary: continue when connected, else sign in or save the pasted token. */
  const next = () => {
    if (!provider) return
    if (provider.connected || provider.provider === 'kubernetes' || provider.provider === 'ssh') return setStep('configure')
    if (provider.oauth && connect.mode === 'oauth') return void signIn()
    if (!connect.token.trim()) return
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
    if (isSsh && (!sshOpts.host || sshPortError)) return
    actions.create.mutate(
      { spec, vaultPassword: vault && !isK8s && vaultPassword && !onPhoneEngine ? vaultPassword : null },
      {
        onSuccess: (n) => {
          if (!onPhoneEngine) {
            setVaultPassword('')
            setVaultConfirm('')
          }
          setCreated(n.name)
          setStep('progress')
        },
      },
    )
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
    // Phone-only: "Connect a model" / "Add my laptop" happen on the node, so it becomes the connected engine first.
    phoneLink.use()
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
      ) : isSsh ? (
        <SshConfigure
          hosts={sshHostsQ.data ?? null}
          loading={sshHostsQ.isPending}
          mode={sshMode}
          onMode={(nextMode) => {
            setSshMode(nextMode)
            resetSshProbe()
          }}
          ssh={ssh}
          onChange={updateSsh}
          onPickHost={pickSshHost}
          portText={sshPortText}
          onPortText={updateSshPort}
          portError={sshPortError}
          name={nodeName}
          nameError={nameError}
          onName={handleName}
          probe={{ result: sshProbeResult, error: sshProbeError, pending: sshProbeM.isPending, onCheck: checkSshServer }}
          vault={vault}
          busy={actions.create.isPending}
          onCreate={create}
          onBack={back}
        />
      ) : phone ? (
        <PhoneConfigureStep
          provider={provider}
          spec={spec}
          sizes={sizes}
          quote={quote}
          nameError={nameError}
          busy={actions.create.isPending}
          onName={handleName}
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
          onName={handleName}
          onRegion={(id) => setRegions((r) => ({ ...r, [provider.provider]: id }))}
          onMemory={setMemory}
          onDesktop={setDesktop}
          vault={vault}
          onCreate={create}
        />
      )}
    </>
  )

  const progressView = created && (
    <>
      <ProgressView name={created} rows={rows} pct={progressPct(rows, progress)} eta={progress?.eta_secs ?? null} error={node?.error ?? null} retrying={actions.destroy.isPending} phone={phone} onRetry={retry} />
      {node?.link && !node.error && <p className="text-[13px] text-fg-tertiary">{LEAVE_HINT}</p>}
    </>
  )
  const linkState = phoneLink.state
  const nodePeerId = node?.peer_id ?? node?.link?.engine ?? null
  const nodeBootstrapping = !!nodePeerId && isPeerBootstrapping(byPeer.get(nodePeerId))
  const doneView =
    node?.link && !node.link.paired && linkState.phase !== 'paired' ? (
      <div data-cloud-link={linkState.phase} className="flex flex-col gap-2 text-sm">
        <p className="font-medium">{created ?? nodeName} is online</p>
        {linkState.phase === 'error' ? (
          <>
            <p className="text-danger">Could not connect this device: {linkState.error}</p>
            <Button variant="outline" onClick={phoneLink.retry}>
              Try again
            </Button>
          </>
        ) : (
          <p className="text-fg-tertiary">Connecting this device to it…</p>
        )}
      </div>
    ) : (
      <>
        <DoneView name={created ?? nodeName} entry={entry} bootstrapping={nodeBootstrapping} onModel={() => finish('providers')} onLaptop={() => finish('machines')} />
        {linkState.phase === 'paired' && linkState.vault === 'unsupported' && <p className="text-[13px] text-fg-tertiary">{VAULT_LATER}</p>}
        {linkState.phase === 'paired' && linkState.vault === 'failed' && <p className="text-[13px] text-danger">{VAULT_FAILED}</p>}
      </>
    )

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
  const needsToken = !!provider && !provider.connected && !isK8s && !isSsh && (!provider.oauth || connect.mode === 'token')
  let pane
  if (step === 'provider' || step === 'choose')
    pane = loading ?? (
      <>
        <ProviderCards providers={providers} phone={false} grid selected={kind} k8sContexts={k8sInfo.data?.contexts.length ?? null} sshHosts={sshHostsQ.data?.hosts.length ?? null} onSelect={select} onUse={(p, id) => actions.importCredential.mutate({ provider: p.provider, id })} using={actions.importCredential.isPending ? (actions.importCredential.variables?.provider ?? null) : null} />
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
            left={provider?.discovered.length && !provider.connected && !isSsh ? `Found in ${provider.discovered[0]?.path}` : null}
            onBack={onClose}
            action={providerAction(provider, connect)}
            disabled={!provider?.available || (needsToken && !connect.token.trim())}
            busy={actions.setToken.isPending}
            onAction={next}
          />
        )}
      </>
    )
  else if (step === 'configure')
    pane = loading ?? (
      <>
        <ProviderCards providers={providers.filter((p) => p.available && (p.connected || p.provider === kind))} phone={false} grid selected={kind} k8sContexts={k8sInfo.data?.contexts.length ?? null} sshHosts={sshHostsQ.data?.hosts.length ?? null} onSelect={select} />
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
