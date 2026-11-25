import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Popout from '@/lib/popout'
import type * as Rpc from '@/lib/rpc'
import type { Preflight } from "@/rpc/bindings/method.sandbox.output"
import type { SandboxInfo } from "@/rpc/bindings/method.sandbox.output"
import { limits, nextVmName, resizeGuard, sandboxLogKey } from '@/store/sandboxes'
import { toCreate } from './SandboxSheets'
import { MEMORY_ALLOCATED_TIP, SandboxVms, openVmTerminal } from './SandboxVms'
import { PREFLIGHT_BLOCKED, PREFLIGHT_OK, SANDBOXES_SIM, SANDBOX_LOG_SIM, simFlag } from './sandbox-queries'

let preflight: Preflight = PREFLIGHT_BLOCKED
let sandboxes: SandboxInfo[] = []
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    sandbox_preflight: async () => preflight,
    sandboxes_list: async () => sandboxes,
    list_sessions: async () => [],
    handoffs_list: async () => [],
    mesh_status: async () => ({ self_id: 'me', self_name: 'me', mesh_name: null, peers: [], lease: { holder: null, reason: '', since_ms: 0 } }),
  }),
}))
const popout = vi.hoisted(() => ({ open: vi.fn(async () => {}) }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), openPopout: popout.open, canPopout: () => true }))

async function render(seed?: (qc: QueryClient) => void): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  seed?.(qc)
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(SandboxVms) as ReactNode }))
  for (let pass = 0; pass < 2; pass++) {
    paint()
    for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  }
  return paint()
}

test('blocked host: every blocker is a sentence with its fix, Set up runtime shows for the installable one, Add is disabled with the first blocker as tooltip', async () => {
  preflight = PREFLIGHT_BLOCKED
  const html = await render()
  expect(html).toContain('does not meet the requirements')
  for (const b of PREFLIGHT_BLOCKED.blockers) {
    expect(html).toContain(b.message)
    expect(html).not.toContain(`>${b.code}<`)
  }
  expect(html).toContain('Enable virtualization in the firmware')
  expect(html).toContain('Set up runtime')
  expect(html).not.toContain('downloads microsandbox')
  // Warnings render but do not change the button.
  expect(html).toContain('more than half of this host')
  expect(html).toMatch(new RegExp(`title="${PREFLIGHT_BLOCKED.blockers[0]?.message}"[^>]*>\\s*<button[^>]*disabled`))
})

test('ok host: green summary with runtime, virtualization, host facts and defaults; Add enabled; no Set up button', async () => {
  preflight = PREFLIGHT_OK
  const html = await render()
  expect(html).toContain('This host can run sandbox VMs')
  expect(html).toContain('microsandbox 0.7.4 (embedded, in /Users/sky/.microsandbox)')
  expect(html).toContain('virtualization via hvf')
  expect(html).toContain('16 cores')
  expect(html).toContain('64 GiB')
  expect(html).toContain('defaults 4 CPUs (up to 16) · 8 GiB (up to 32 GiB) · 20 GiB disk')
  expect(html).not.toContain('Set up runtime')
  expect(html).not.toMatch(/aria-label="Add a sandbox VM"[^>]*disabled/)
  // The docker warning is warning-blue text, not a blocker.
  expect(html).toContain('Docker is not installed')
})

test('embedded runtime not extracted yet: green, says first use sets it up without a download, offers Set up runtime, Add stays enabled', async () => {
  preflight = { ...PREFLIGHT_OK, runtime: { state: 'embedded', version: '0.7.4', extracted: false, home: '/Users/sky/.microsandbox' } }
  const html = await render()
  expect(html).toContain('This host can run sandbox VMs')
  expect(html).toContain('microsandbox 0.7.4 embedded — set up on first use (no download)')
  expect(html).toContain('Set up runtime')
  expect(html).not.toMatch(/aria-label="Add a sandbox VM"[^>]*disabled/)
})

test('the create sheet starts from Preflight.defaults, the next free vm-N name, and sends only what differs', () => {
  const d = PREFLIGHT_OK.defaults
  expect(nextVmName([])).toBe('vm-1')
  expect(nextVmName([{ name: 'vm-1' }, { name: 'lab' }, { name: 'vm-3' }])).toBe('vm-2')
  const lim = limits(PREFLIGHT_OK)
  expect(lim.cpus).toEqual([1, 16])
  expect(lim.memory_mib[1]).toBe(32 * 1024)
  const same = toCreate({ name: ' vm-1 ', res: d, shares: [], desktop: true, network: 'public_host', image: undefined, autostart: true }, d)
  expect(same).toEqual({ name: 'vm-1', shares: [], cpus: undefined, memory_mib: undefined, root_disk_mib: undefined, max_cpus: undefined, max_memory_mib: undefined, desktop: undefined, network: undefined, image: undefined, autostart: undefined })
  const changed = toCreate({ name: 'lab', res: { ...d, cpus: 8, root_disk_mib: 40960 }, shares: [{ host: '/x/Dev', guest: '/shared/Dev', readonly: true }], desktop: false, network: 'public_host_private', image: { kind: 'oci', ref: 'r' }, autostart: false }, d)
  expect(changed.cpus).toBe(8)
  expect(changed.root_disk_mib).toBe(40960)
  expect(changed.memory_mib).toBeUndefined()
  expect(changed.desktop).toBe(false)
  expect(changed.network).toBe('public_host_private')
  expect(changed.autostart).toBe(false)
  expect(changed.shares?.[0]?.guest).toBe('/shared/Dev')
})

test('the resize guard mirrors the server: memory floor = use + 512 MiB, at least 1 CPU, disk never shrinks', () => {
  const lab = SANDBOXES_SIM[0]!
  const cur = lab.vm!.resources
  const m = lab.vm!.metrics!
  expect(resizeGuard(cur, cur, m)).toBeNull()
  expect(resizeGuard({ ...cur, cpus: 0 }, cur, m)).toBe('A VM needs at least 1 CPU.')
  // 2.6 GiB in use → floor 2.6 GiB + 512 MiB ≈ 3.1 GiB: 3072 MiB is refused, 4096 allowed.
  expect(resizeGuard({ ...cur, memory_mib: 3072 }, cur, m)).toMatch(/cannot go below/)
  expect(resizeGuard({ ...cur, memory_mib: 4096 }, cur, m)).toBeNull()
  expect(resizeGuard({ ...cur, root_disk_mib: cur.root_disk_mib - 1 }, cur, m)).toMatch(/only grow/)
  // Without a metrics sample only the structural rules apply.
  expect(resizeGuard({ ...cur, memory_mib: 1024 }, cur, null)).toBeNull()
})

test('simFlag reads only the two sandbox fixtures', () => {
  expect(simFlag('?sim=sandbox-blocked')).toBe('sandbox-blocked')
  expect(simFlag('?sim=sandbox-ok')).toBe('sandbox-ok')
  expect(simFlag('?sim=idle')).toBeNull()
  expect(simFlag('')).toBeNull()
})

test('Terminal: sandbox_terminal(name) answers the PTY job and the 55 pop-out opens on it', async () => {
  const sandbox_terminal = vi.fn(async (name: string) => ({ session: `sandbox:${name}`, job: 'job-7' }))
  await openVmTerminal({ sandbox_terminal }, 'lab')
  expect(sandbox_terminal).toHaveBeenCalledWith('lab')
  expect(popout.open).toHaveBeenCalledWith({ kind: 'pty', session: 'sandbox:lab', job: 'job-7' })
})

test('the memory gauge says "allocated" of the hypervisor limit and carries the MemTotal tooltip; rows without a sample name the allocation too', async () => {
  preflight = PREFLIGHT_OK
  sandboxes = SANDBOXES_SIM
  const html = await render()
  expect(html).toContain('2.6 GiB of 8 GiB allocated')
  expect(html).toContain('4 GiB allocated')
  expect(html).toContain(`title="${MEMORY_ALLOCATED_TIP.replace("'", '&#x27;')}"`)
  // The stopped VM with pending shares says so; the failed one names its step.
  expect(html).toContain('restart to apply shares')
  expect(html).toContain('Failed at <span class="font-medium">pair</span>')
})

test('a starting row shows the `start` progress lines of its log (F2 streams them while Start recreates)', async () => {
  preflight = PREFLIGHT_OK
  sandboxes = SANDBOXES_SIM
  const html = await render((qc) => qc.setQueryData(sandboxLogKey('ci'), SANDBOX_LOG_SIM.ci))
  expect(html).toContain('start      snapshot ci:auto-restart-3 (disk)')
  expect(html).toContain('waiting for the guest agent (70%)')
  // A running row never shows a log.
  expect(html).not.toContain('snapshot lab:')
})
