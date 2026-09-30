// Settings › Mesh › Sandbox VMs: the queries the section reads, the mutations its rows call (every one through
// `useAction`), and the `?sim=sandbox-blocked` / `?sim=sandbox-ok` fixtures that let the preflight card be reviewed
// without a runtime (the repo's `?sim=` idiom, `lib/talk-sim.ts`; the general `?mock=1` switch is 67's).

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useAction } from '@/components/settings/shared'
import { keys as taskKeys } from '@/components/tasks/queries'
import { queryKeys, useApi } from '@/lib/rpc'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { Handoff } from '@/rpc/bindings/Handoff'
import type { Mount } from '@/rpc/bindings/Mount'
import type { Preflight } from '@/rpc/bindings/Preflight'
import type { ResourceChange } from '@/rpc/bindings/ResourceChange'
import type { SandboxCreate } from '@/rpc/bindings/SandboxCreate'
import type { SandboxInfo } from '@/rpc/bindings/SandboxInfo'
import type { SecretRequest } from '@/rpc/bindings/SecretRequest'
import { HANDOFFS_KEY, PREFLIGHT_KEY, type ProgressLine, SANDBOXES_KEY, sandboxLogKey } from '@/store/sandboxes'

export type Sim = 'sandbox-blocked' | 'sandbox-ok' | null

/** The `?sim=` flag of this window, or `null`. */
export function simFlag(search = typeof location === 'undefined' ? '' : location.search): Sim {
  const v = new URLSearchParams(search).get('sim')
  return v === 'sandbox-blocked' || v === 'sandbox-ok' ? v : null
}

const HOST = { os: 'macos', arch: 'aarch64', cores: 16, mem_bytes: 64 * 1024 ** 3, disk_free_bytes: 412 * 1024 ** 3 }
const DEFAULTS = { cpus: 4, max_cpus: 16, memory_mib: 8192, max_memory_mib: 32768, root_disk_mib: 20480 }

export const PREFLIGHT_OK: Preflight = {
  runtime: { state: 'installed', version: '0.7.4', home: '/Users/sky/.microsandbox', libkrunfw: '4.9.0' },
  virt: { state: 'ok', via: 'hvf' },
  host: HOST,
  defaults: DEFAULTS,
  blockers: [],
  warnings: [{ code: 'docker_missing', message: 'Docker is not installed; the dev image path (`just vm-image-dev`) needs it.', fix: 'brew install --cask docker', installable: false }],
  checked_at_ms: Date.now(),
}

export const PREFLIGHT_BLOCKED: Preflight = {
  ...PREFLIGHT_OK,
  runtime: { state: 'missing' },
  virt: { state: 'unsupported', why: '/dev/kvm missing' },
  host: { ...HOST, os: 'linux', arch: 'x86_64', disk_free_bytes: 6 * 1024 ** 3 },
  blockers: [
    {
      code: 'no_kvm',
      message: 'Virtualization is not available: /dev/kvm is missing.',
      fix: 'Enable virtualization in the firmware, or nested virtualization in the outer hypervisor.',
      installable: false,
    },
    { code: 'runtime_missing', message: 'The microsandbox runtime is not installed.', fix: 'brew install superradcompany/tap/microsandbox', installable: true },
    { code: 'disk_low', message: '6 GiB free; a VM needs its root disk plus the image plus 2 GiB (about 25 GiB).', fix: null, installable: false },
  ],
  warnings: [{ code: 'memory_low', message: 'The default memory (8 GiB) is more than half of this host.', fix: null, installable: false }],
}

const NOW = Date.now()
export const SANDBOXES_SIM: SandboxInfo[] = [
  {
    name: 'lab',
    backend: 'vm',
    state: { state: 'running' },
    engine: 'engine-lab-0001',
    created_ms: NOW - 86_400_000,
    updated_ms: NOW,
    vm: {
      image: { kind: 'oci', ref: 'ghcr.io/basedatum/theuth-vm:2.1.2' },
      resources: DEFAULTS,
      shares: [{ host: '/Users/sky/Develop', guest: '/shared/Develop', readonly: false }],
      desktop: { enabled: true, geometry: '1440x900', host_port: 61234 },
      network: 'public_host',
      autostart: true,
      pending: { shares: true, ports: false, resources: false },
      metrics: {
        cpu_percent: 37,
        memory_bytes: 2.6 * 1024 ** 3,
        memory_limit_bytes: 8 * 1024 ** 3,
        disk_used_bytes: 7.2 * 1024 ** 3,
        disk_free_bytes: 12.8 * 1024 ** 3,
        net_rx_bytes: 0,
        net_tx_bytes: 0,
        uptime_secs: 3600,
        at_ms: NOW,
      },
      snapshots: [{ id: 'lab:clean', label: 'clean', full: false, at_ms: NOW - 3_600_000, bytes: 3.1 * 1024 ** 3, auto: false }, { id: 'lab:auto-retry-1', label: 'auto-retry-1', full: false, at_ms: NOW - 7_200_000, bytes: null, auto: true }],
      mesh_port: 61235,
      host_engine: 'engine-host-0000',
    },
  },
  {
    name: 'vm-2',
    backend: 'vm',
    state: { state: 'failed', step: 'pair', error: 'mesh_add timed out after 120 s' },
    engine: null,
    created_ms: NOW - 600_000,
    updated_ms: NOW - 60_000,
    vm: {
      image: { kind: 'bootstrap', base: 'ghcr.io/basedatum/theuth-vm-base:trixie', script: 'bootstrap.sh' },
      resources: { ...DEFAULTS, cpus: 2, memory_mib: 4096 },
      shares: [],
      desktop: null,
      network: 'public_only',
      autostart: false,
      pending: { shares: false, ports: false, resources: false },
      metrics: null,
      snapshots: [],
      mesh_port: null,
      host_engine: 'engine-host-0000',
    },
  },
  {
    name: 'staging',
    backend: 'vm',
    state: { state: 'stopped' },
    engine: 'engine-staging-0002',
    created_ms: NOW - 172_800_000,
    updated_ms: NOW - 7_200_000,
    vm: {
      image: { kind: 'oci', ref: 'ghcr.io/basedatum/theuth-vm:2.1.2' },
      resources: { ...DEFAULTS, cpus: 2, memory_mib: 4096 },
      shares: [{ host: '/Users/sky/Sites', guest: '/shared/Sites', readonly: true }],
      desktop: { enabled: true, geometry: '1280x800', host_port: null },
      network: 'public_host',
      autostart: false,
      pending: { shares: true, ports: false, resources: false },
      metrics: null,
      snapshots: [{ id: 'staging:clean', label: 'clean', full: false, at_ms: NOW - 86_400_000, bytes: 2.4 * 1024 ** 3, auto: false }],
      mesh_port: null,
      host_engine: 'engine-host-0000',
    },
  },
  {
    name: 'ci',
    backend: 'vm',
    state: { state: 'starting' },
    engine: 'engine-ci-0003',
    created_ms: NOW - 259_200_000,
    updated_ms: NOW - 2_000,
    vm: {
      image: { kind: 'oci', ref: 'ghcr.io/basedatum/theuth-vm:2.1.2' },
      resources: DEFAULTS,
      shares: [],
      desktop: null,
      network: 'public_only',
      autostart: true,
      pending: { shares: false, ports: false, resources: false },
      metrics: null,
      snapshots: [],
      mesh_port: null,
      host_engine: 'engine-host-0000',
    },
  },
]

/** What F2's `Start` (recreate) streams as `Progress { step: "start" }` (smoke bug 8); `ci` shows it while `starting`. */
export const SANDBOX_LOG_SIM: Record<string, ProgressLine[]> = {
  ci: [
    { step: 'start', line: 'snapshot ci:auto-restart-3 (disk)', pct: null },
    { step: 'start', line: 'removing the stopped sandbox', pct: null },
    { step: 'start', line: 'booting ghcr.io/basedatum/theuth-vm:2.1.2 with 4 CPUs · 8 GiB', pct: 40 },
    { step: 'start', line: 'waiting for the guest agent', pct: 70 },
  ],
}

/** An unresolved hand-off raised on `lab`'s engine (79 §6.6). */
export const HANDOFFS_SIM: Handoff[] = [
  { id: 'h-sim-1', engine: 'engine-lab-0001', session: 'sess-lab-01a0f02b', agent: 'researcher', reason: 'the login page wants a 2FA code', url: 'https://accounts.example.com/2fa', created_ms: NOW - 120_000, resolved_ms: null, outcome: null, hlc: null },
]

/** Two unanswered `secret_requests` rows: a single field raised on `lab`, a form raised on this host (79 §6.6). */
export const SECRET_REQUESTS_SIM: SecretRequest[] = [
  { id: '0199aaaa-0000-7000-8000-00000000sim1', engine: 'engine-lab-0001', session: 'sess-lab-01a0f02b', title: null, fields: [{ name: 'GITHUB_TOKEN', label: 'a fine-grained token with repo scope', masked: true, current: 1 }], created_ms: NOW - 90_000, answered_ms: null, outcome: null, hlc: null },
  { id: '0199aaaa-0000-7000-8000-00000000sim2', engine: 'engine-host-0000', session: 'sess-host-77aa11bb', title: 'Postgres login', fields: [{ name: 'PGUSER', label: 'User', masked: false, current: null }, { name: 'PGPASSWORD', label: 'Password', masked: true, current: null }], created_ms: NOW - 30_000, answered_ms: null, outcome: null, hlc: null },
]

/** The inbox as `attention_list` derives it from the two fixtures above (roster `attention.rs`: titles, `detail` = engine name or id). */
export const ATTENTION_SIM: AttentionItem[] = [
  { id: 'secret:0199aaaa-0000-7000-8000-00000000sim2', kind: 'secret', title: '77aa11bb asks for a secret: Postgres login', detail: 'engine-host-0000', at_ms: NOW - 30_000, task: null, agent: null, session: 'sess-host-77aa11bb', approval: null, decision: null },
  { id: 'secret:0199aaaa-0000-7000-8000-00000000sim1', kind: 'secret', title: '01a0f02b asks for a secret: GITHUB_TOKEN', detail: 'lab', at_ms: NOW - 90_000, task: null, agent: null, session: 'sess-lab-01a0f02b', approval: null, decision: null },
  { id: 'handoff:h-sim-1', kind: 'handoff', title: 'researcher needs you: the login page wants a 2FA code', detail: 'lab', at_ms: NOW - 120_000, task: null, agent: 'researcher', session: 'sess-lab-01a0f02b', approval: null, decision: null },
]

/** `sandboxes_list`, patched live by `SandboxEvent` (`store/sandboxes.ts`); the poll is the fallback. */
export function useSandboxes() {
  const api = useApi()
  const sim = simFlag()
  return useQuery({
    queryKey: SANDBOXES_KEY,
    queryFn: () => (sim ? SANDBOXES_SIM : api.sandboxes_list()),
    refetchInterval: 60_000,
  })
}

/** `sandbox_preflight` on mount and on every window focus (79 §5.1: "runs on the Mesh page's first paint"). */
export function usePreflight() {
  const api = useApi()
  const sim = simFlag()
  return useQuery({
    queryKey: PREFLIGHT_KEY,
    queryFn: () => (sim === 'sandbox-blocked' ? PREFLIGHT_BLOCKED : sim === 'sandbox-ok' ? PREFLIGHT_OK : api.sandbox_preflight()),
    staleTime: 0,
    refetchOnWindowFocus: 'always',
  })
}

/** The bring-up / install log of one name (`runtime` for the installer); fed only by `Progress` events (the fixture seeds `?sim`). */
export function useSandboxLog(name: string): ProgressLine[] {
  const sim = simFlag()
  const q = useQuery({ queryKey: sandboxLogKey(name), queryFn: () => (sim && SANDBOX_LOG_SIM[name]) || ([] as ProgressLine[]), staleTime: Infinity, gcTime: Infinity })
  return q.data ?? []
}

/** `handoffs_list`, unresolved rows are what the pop-out's banner and the HUD show. */
export function useHandoffs(enabled = true) {
  const api = useApi()
  const sim = simFlag()
  return useQuery({ queryKey: HANDOFFS_KEY, queryFn: () => (sim ? HANDOFFS_SIM : api.handoffs_list()), enabled, refetchInterval: 15_000 })
}

export const unresolved = (rows: readonly Handoff[] | undefined, engine: string | null | undefined): Handoff | undefined =>
  engine ? rows?.find((h) => h.engine === engine && !h.resolved_ms) : undefined

/** Every mutation of the section; each refetches the list (the `Changed` event usually lands first). */
export function useSandboxActions() {
  const api = useApi()
  const qc = useQueryClient()
  const keys = [SANDBOXES_KEY] as const
  return {
    create: useAction((spec: SandboxCreate) => api.sandbox_create(spec), {
      keys,
      // The row shows at once in `creating`; the `Changed` stream then owns it.
      onSuccess: (info) => qc.setQueryData<SandboxInfo[]>(SANDBOXES_KEY, (rows) => (rows?.some((r) => r.name === info.name) ? rows : [...(rows ?? []), info])),
    }),
    install: useAction(() => api.sandbox_install_runtime(), { keys: [PREFLIGHT_KEY] }),
    retry: useAction((name: string) => api.sandbox_retry(name), { keys }),
    start: useAction((name: string) => api.sandbox_start(name), { keys }),
    stop: useAction((name: string) => api.sandbox_stop(name, null, null), { keys }),
    pause: useAction((name: string) => api.sandbox_pause(name), { keys }),
    resume: useAction((name: string) => api.sandbox_resume(name), { keys }),
    resize: useAction(({ name, change }: { name: string; change: ResourceChange }) => api.sandbox_resize(name, change), { keys }),
    setShares: useAction(({ name, shares }: { name: string; shares: Mount[] }) => api.sandbox_set_shares(name, shares), { keys }),
    snapshot: useAction(({ name, label, full }: { name: string; label: string; full: boolean }) => api.sandbox_snapshot(name, label, full), { keys }),
    restore: useAction(({ name, id }: { name: string; id: string }) => api.sandbox_restore(name, id), { keys }),
    destroy: useAction(({ name, keep }: { name: string; keep: boolean }) => api.sandbox_destroy(name, keep), { keys: [SANDBOXES_KEY, taskKeys.mesh, queryKeys.sessions] }),
  }
}
