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
import type { Packet } from '@/rpc/bindings/Packet'
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
  runtime: { state: 'embedded', version: '0.7.4', extracted: true, home: '/Users/sky/.microsandbox' },
  virt: { state: 'ok', via: 'hvf' },
  host: HOST,
  defaults: DEFAULTS,
  blockers: [],
  warnings: [{ code: 'docker_missing', message: 'Docker is not installed; the dev image path (`just vm-image-dev`) needs it.', fix: 'brew install --cask docker', installable: false }],
  checked_at_ms: Date.now(),
}

export const PREFLIGHT_BLOCKED: Preflight = {
  ...PREFLIGHT_OK,
  runtime: { state: 'embedded', version: '0.7.4', extracted: false, home: '/root/.microsandbox' },
  virt: { state: 'unsupported', why: '/dev/kvm missing' },
  host: { ...HOST, os: 'linux', arch: 'x86_64', disk_free_bytes: 6 * 1024 ** 3 },
  blockers: [
    {
      code: 'no_kvm',
      message: 'Virtualization is not available: /dev/kvm is missing.',
      fix: 'Enable virtualization in the firmware, or nested virtualization in the outer hypervisor.',
      installable: false,
    },
    { code: 'runtime_extract_failed', message: 'The embedded microsandbox runtime could not be set up (read-only file system).', fix: 'check that the home is writable, then Set up runtime again', installable: true },
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

/** 101: the two mock-67 tasks the sim's decision and review hang off (`tasks/mock.ts` ids 2 and 3; `?mock=1` serves them). */
const SIM_TASK_DECISION = '0192a000-0000-7000-8000-000000000002'
const SIM_TASK_REVIEW = '0192a000-0000-7000-8000-000000000003'

/** The inbox as `attention_list` derives it from the fixtures (roster `attention.rs`: titles, `detail` = engine name or id; 101: the first line of the packet). */
export const ATTENTION_SIM: AttentionItem[] = [
  { id: `decision:${SIM_TASK_DECISION}:decision:apr-7f3a`, kind: 'decision', title: 'Rotate the Grafana API token', detail: 'Which rotation path for the Grafana token?', at_ms: NOW - 20_000, task: SIM_TASK_DECISION, agent: 'ops', session: null, approval: null, decision: 'decision:apr-7f3a', packet: 'decision:apr-7f3a' },
  { id: 'secret:0199aaaa-0000-7000-8000-00000000sim2', kind: 'secret', title: '77aa11bb asks for a secret: Postgres login', detail: 'engine-host-0000', at_ms: NOW - 30_000, task: null, agent: null, session: 'sess-host-77aa11bb', approval: null, decision: null, packet: null },
  { id: `review:${SIM_TASK_REVIEW}`, kind: 'review', title: 'Write the Q4 release notes', detail: 'Release notes for 2.4: three sections, one diagram, a diff of the changelog.', at_ms: NOW - 45_000, task: SIM_TASK_REVIEW, agent: 'writer', session: null, approval: null, decision: null, packet: `review:${SIM_TASK_REVIEW}` },
  { id: 'secret:0199aaaa-0000-7000-8000-00000000sim1', kind: 'secret', title: '01a0f02b asks for a secret: GITHUB_TOKEN', detail: 'lab', at_ms: NOW - 90_000, task: null, agent: null, session: 'sess-lab-01a0f02b', approval: null, decision: null, packet: null },
  { id: 'handoff:h-sim-1', kind: 'handoff', title: 'researcher needs you: the login page wants a 2FA code', detail: 'lab', at_ms: NOW - 120_000, task: null, agent: 'researcher', session: 'sess-lab-01a0f02b', approval: null, decision: null, packet: null },
]

/**
 * 101b R1: the sim has no live document, and the browser cannot `doc_create(kind="board")`, so these point at the
 * board notes `doc_list("board")` returned on the dev daemon (2026-10-06). Elsewhere `PacketBoard` shows its
 * "could not be opened" line and the summary — also a state worth seeing.
 */
const SIM_BOARD_DECISION = 'agqpgxemf5zcdnb45r3i6ryn3e'
const SIM_BOARD_REVIEW = 'agqol5d3h52ohgmj56t666drem'

/** 101 §3: the packets behind the two items above, served by `usePacket` under `?sim=`. */
export const PACKET_SIM: Record<string, Packet> = {
  'decision:apr-7f3a': {
    key: 'decision:apr-7f3a',
    task: SIM_TASK_DECISION,
    kind: 'decision',
    by: 'agent:ops',
    at_ms: NOW - 20_000,
    question: 'Which rotation path for the Grafana token?',
    summary: `The Grafana service-account token in \`infra/grafana/.env\` expires on Friday. Three dashboards and the on-call alert rule authenticate with it, so a rotation that misses one consumer silently stops alerts.

I can rotate in place (one token, one restart, a two-minute alerting gap), run two tokens side by side for a day and retire the old one once every consumer has picked up the new value (no gap, two secrets to track), or move the alert rule onto its own token first so dashboards and alerting never share a credential again (the cleanest end state, one extra Friday change).

\`\`\`mermaid
flowchart LR
  A[new token issued] --> B{rotation path}
  B -->|in place| C[restart · 2 min gap]
  B -->|side by side| D[both valid 24 h] --> E[retire old]
  B -->|split first| F[alert rule gets own token] --> C
\`\`\`

Whichever path, the new value lands in the \`GRAFANA_TOKEN\` secret and the runbook gets the date.`,
    options: [
      { label: 'Rotate in place', description: 'One token, one restart tonight; alerting is blind for about two minutes.' },
      { label: 'Side by side for a day', description: 'Issue the new token now, retire the old one tomorrow after every consumer moved. No gap.' },
      { label: 'Split alerting first', description: 'Give the alert rule its own token, then rotate the dashboards token in place. Two changes.' },
    ],
    recommended: 1,
    pointers: [
      { kind: 'url', ref: 'https://grafana.com/docs/grafana/latest/administration/service-accounts/', title: 'Grafana: service account tokens', note: 'Two tokens per service account are allowed, which the side-by-side path relies on.', text: null, root: null },
      {
        kind: 'code',
        ref: 'infra/grafana/alerting.tf#L12-L19',
        title: 'The alert rule reads the same token as the dashboards',
        note: 'Line 15 is the shared reference; splitting means a second data source here.',
        text: `resource "grafana_rule_group" "oncall" {
  name             = "oncall"
  folder_uid       = grafana_folder.sre.uid
  interval_seconds = 60
  rule {
    datasource_uid = grafana_data_source.prom.uid # token: var.grafana_token
  }
}`,
        root: '/srv/repos/sre',
      },
    ],
    checks: [],
    board: SIM_BOARD_DECISION,
  },
  [`review:${SIM_TASK_REVIEW}`]: {
    key: `review:${SIM_TASK_REVIEW}`,
    task: SIM_TASK_REVIEW,
    kind: 'review',
    by: 'agent:writer',
    at_ms: NOW - 45_000,
    question: null,
    summary: `Release notes for 2.4: three sections (Highlights, Changes, Upgrade notes), one diagram of the new sync flow, and the changelog entry. I kept every item to one line and linked the design notes instead of restating them.

Two things I was unsure about: whether the pricing change belongs in Highlights or Changes (it is in Highlights), and the exact wording of the upgrade warning for self-hosted installs.`,
    options: [],
    recommended: null,
    pointers: [
      {
        kind: 'diff',
        ref: 'main..notes/2.4',
        title: 'CHANGELOG and the notes page',
        note: 'The CHANGELOG entry mirrors the Highlights section; check the self-hosted warning wording.',
        text: `diff --git a/CHANGELOG.md b/CHANGELOG.md
index 3f1a2b4..9c8d7e6 100644
--- a/CHANGELOG.md
+++ b/CHANGELOG.md
@@ -1,5 +1,12 @@
 # Changelog
 
+## 2.4.0 — 2026-10-09
+
+- Sync runs through the relay when both engines are behind NAT.
+- Usage-based pricing for sandbox VMs (see the pricing page).
+- Self-hosted installs: run \`theuth migrate\` before starting 2.4 — the tasks store gains a table.
+
 ## 2.3.1 — 2026-09-20
 
-- Fixed the flaky deploy on build-A.
+- Fixed the flaky deploy on build-A (ops).
diff --git a/site/releases/2.4.md b/site/releases/2.4.md
new file mode 100644
--- /dev/null
+++ b/site/releases/2.4.md
@@ -0,0 +1,9 @@
+# 2.4
+
+## Highlights
+
+Sync through the relay, usage-based VM pricing, and a migration step for self-hosted installs.
+
+## Upgrade notes
+
+Self-hosted: run \`theuth migrate\` first.
`,
        root: '/srv/repos/site',
      },
      {
        kind: 'code',
        ref: 'site/releases/2.4.md#L7-L9',
        title: 'The upgrade warning',
        note: 'Is "first" strong enough, or should it say what breaks without it?',
        text: `## Upgrade notes

Self-hosted: run \`theuth migrate\` first.`,
        root: '/srv/repos/site',
      },
      { kind: 'artifact', ref: 'call-sim-sync-flow', title: 'Sync flow diagram', note: 'The one diagram in the notes; check the relay hop is labelled.', text: null, root: null },
    ],
    checks: [
      { text: 'Three sections, one line per item', done: true, note: null },
      { text: 'Diagram of the sync flow', done: true, note: 'rendered from the mermaid artifact' },
      { text: 'Pricing change reviewed by finance', done: false, note: 'asked in #finance, no answer yet' },
    ],
    board: SIM_BOARD_REVIEW,
  },
}

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
