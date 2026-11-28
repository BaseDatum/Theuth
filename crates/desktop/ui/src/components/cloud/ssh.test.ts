import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { AlertDialog } from '@/components/ui/alert-dialog'
import { DestroyBody, NodeRow, placeChip } from './CloudNodesSettings'
import { formatSshOsArch, stageRows } from './format'
import { SshConfigure, hostSubtitle, parseSshPort, suggestSshNodeName } from './SshConfigure'
import { sshApi } from './sshApi'
import { ConnectPanel, providerAction, providerDetail } from './steps'
import type { CloudNode, NodeSpec, ProviderInfo, SshHostEntry, SshHosts, SshOptions, SshProbe } from './types'

const html = (el: ReactNode) => renderToStaticMarkup(el as never)

const HOSTS: SshHostEntry[] = [
  { alias: 'prod-box', hostname: '10.0.0.10', user: 'ubuntu', port: 2222, identity: '~/.ssh/id_prod', proxy_jump: 'bastion' },
  { alias: 'mac-mini', hostname: '192.168.1.20', user: 'sky', port: null, identity: null, proxy_jump: null },
  { alias: 'win-dev', hostname: '192.168.1.30', user: 'admin', port: 22, identity: null, proxy_jump: null },
]

const SSH_HOSTS: SshHosts = { ssh_available: true, hosts: HOSTS }

const SSH_PROVIDER: ProviderInfo = {
  provider: 'ssh',
  label: 'Install via SSH',
  available: true,
  beta: false,
  desktop_only: true,
  connected: true,
  source: null,
  discovered: [{ id: 'ssh:config', label: 'SSH config', detail: '3 hosts', path: '~/.ssh/config' }],
  oauth: false,
  sizes: [],
  regions: [],
  volume_usd_per_gb: null,
}

const sshNode = (n: Partial<CloudNode> = {}): CloudNode => ({
  name: 'prod-box',
  provider: 'ssh',
  region: null,
  region_label: null,
  memory_gb: null,
  size_slug: null,
  volume_gb: null,
  monthly_usd: null,
  state: 'ready',
  stage: null,
  error: null,
  provider_state: null,
  ip: null,
  peer_id: 'peer-ssh-1',
  reachable: true,
  path: 'direct',
  version: '2.31.2',
  auto_update: true,
  created_ms: null,
  k8s: null,
  ssh: { host: 'prod-box', user: 'ubuntu', port: 2222, identity: '~/.ssh/id_prod', name: 'prod-box' },
  desktop: false,
  vault: null,
  link: null,
  ...n,
})

test('provider detail, no token panel, and Continue action for SSH', () => {
  expect(providerDetail(SSH_PROVIDER, false, null, 3)).toBe('3 hosts in ~/.ssh/config')
  expect(providerDetail(SSH_PROVIDER, false, null, 1)).toBe('1 host in ~/.ssh/config')
  expect(providerDetail({ ...SSH_PROVIDER, discovered: [] }, false, null, 0)).toBe('A machine you already SSH to')
  expect(providerDetail(SSH_PROVIDER, false, null, null)).toBe('3 hosts in ~/.ssh/config')

  const connect = { mode: 'oauth' as const, waiting: false, error: null, token: '' }
  expect(html(createElement(ConnectPanel, { p: SSH_PROVIDER, connect, onToken: () => {}, onPasteInstead: () => {}, onCancel: () => {} }))).toBe('')
  expect(providerAction(SSH_PROVIDER, connect)).toBe('Continue')
})

test('SshConfigure renders ~/.ssh/config host list with subtitles and selected state', () => {
  expect(hostSubtitle(HOSTS[0]!)).toBe('ubuntu@10.0.0.10:2222 · via bastion')
  expect(hostSubtitle(HOSTS[1]!)).toBe('sky@192.168.1.20')

  const out = html(
    createElement(SshConfigure, {
      hosts: SSH_HOSTS,
      mode: 'config',
      onMode: () => {},
      ssh: { host: 'prod-box', user: null, port: null, identity: null, name: null },
      onChange: () => {},
      onPickHost: () => {},
      portText: '',
      onPortText: () => {},
      portError: null,
      name: 'prod-box',
      nameError: null,
      onName: () => {},
      probe: { result: null, error: null, pending: false, onCheck: () => {} },
      vault: { password: '', confirm: '', onPassword: () => {}, onConfirm: () => {} },
      busy: false,
      onCreate: () => {},
    }),
  )

  expect(out).toContain('~/.ssh/config (3)')
  expect(out).toContain('data-ssh-host="prod-box"')
  expect(out).toContain('aria-checked="true" data-ssh-host="prod-box"')
  expect(out).toContain('ubuntu@10.0.0.10:2222 · via bastion')
  expect(out).toContain('data-ssh-host="mac-mini"')
  expect(out).toContain('Check server')
  expect(out).toContain('Vault protection')
  expect(out).toContain('Install and join')
})

test('SshConfigure manual entry renders host, user, port (default 22), identity file, and port validation', () => {
  expect(parseSshPort('')).toEqual({ port: null, error: null })
  expect(parseSshPort('2222')).toEqual({ port: 2222, error: null })
  expect(parseSshPort('0').error).not.toBeNull()
  expect(parseSshPort('70000').error).not.toBeNull()
  expect(parseSshPort('abc').error).not.toBeNull()

  const out = html(
    createElement(SshConfigure, {
      hosts: { ssh_available: true, hosts: [] },
      mode: 'manual',
      onMode: () => {},
      ssh: { host: '10.0.0.5', user: 'deploy', port: 2222, identity: '~/.ssh/id_ed25519', name: null },
      onChange: () => {},
      onPickHost: () => {},
      portText: '2222',
      onPortText: () => {},
      portError: null,
      name: 'deploy-box',
      nameError: null,
      onName: () => {},
      probe: { result: null, error: null, pending: false, onCheck: () => {} },
      vault: null,
      busy: false,
      onCreate: () => {},
    }),
  )

  expect(out).toContain('aria-label="Host"')
  expect(out).toContain('value="10.0.0.5"')
  expect(out).toContain('aria-label="User"')
  expect(out).toContain('value="deploy"')
  expect(out).toContain('aria-label="Port"')
  expect(out).toContain('placeholder="22"')
  expect(out).toContain('aria-label="Identity file"')
  expect(out).toContain('value="~/.ssh/id_ed25519"')
  expect(out).not.toMatch(/<button[^>]*disabled=""[^>]*>Install and join<\/button>/)

  const invalid = html(
    createElement(SshConfigure, {
      hosts: { ssh_available: true, hosts: [] },
      mode: 'manual',
      onMode: () => {},
      ssh: { host: '10.0.0.5', user: null, port: null, identity: null, name: null },
      onChange: () => {},
      onPickHost: () => {},
      portText: '99999',
      onPortText: () => {},
      portError: 'Port must be between 1 and 65535',
      name: 'deploy-box',
      nameError: null,
      onName: () => {},
      probe: { result: null, error: null, pending: false, onCheck: () => {} },
      vault: null,
      busy: false,
      onCreate: () => {},
    }),
  )
  expect(invalid).toContain('Port must be between 1 and 65535')
  expect(invalid).toMatch(/<button[^>]*disabled=""[^>]*>Install and join<\/button>/)
})

test('SshConfigure displays probe ok (OS · arch) and probe error', () => {
  expect(formatSshOsArch('linux', 'x86_64')).toBe('Linux · x86_64')
  expect(formatSshOsArch('macos', 'aarch64')).toBe('macOS · aarch64')
  expect(formatSshOsArch('windows', 'x86_64')).toBe('Windows · x86_64')

  const base = {
    hosts: SSH_HOSTS,
    mode: 'config' as const,
    onMode: () => {},
    ssh: { host: 'prod-box', user: null, port: null, identity: null, name: null },
    onChange: () => {},
    onPickHost: () => {},
    portText: '',
    onPortText: () => {},
    portError: null,
    name: 'prod-box',
    nameError: null,
    onName: () => {},
    vault: null,
    busy: false,
    onCreate: () => {},
  }

  const okProbe: SshProbe = { os: 'linux', arch: 'x86_64', ok: true, error: null }
  const okOut = html(createElement(SshConfigure, { ...base, probe: { result: okProbe, error: null, pending: false, onCheck: () => {} } }))
  expect(okOut).toContain('data-ssh-probe="ok"')
  expect(okOut).toContain('Linux · x86_64')

  const errProbe: SshProbe = { os: 'macos', arch: 'x86_64', ok: false, error: 'macOS Intel (x86_64) is not supported' }
  const errOut = html(createElement(SshConfigure, { ...base, probe: { result: errProbe, error: null, pending: false, onCheck: () => {} } }))
  expect(errOut).toContain('data-ssh-probe="error"')
  expect(errOut).toContain('macOS Intel (x86_64) is not supported')
})

test('sshApi calls cloud_ssh_hosts and cloud_ssh_probe with positional params, and create payload carries SshOptions', async () => {
  const call = vi.fn(async (method: string, params: unknown[]) => {
    if (method === 'cloud_ssh_hosts') return SSH_HOSTS
    if (method === 'cloud_ssh_probe') return { os: 'linux', arch: 'aarch64', ok: true, error: null } satisfies SshProbe
    throw new Error(`unexpected ${method} ${JSON.stringify(params)}`)
  })
  const api = sshApi({ call: call as never })

  const hosts = await api.hosts()
  expect(hosts.ssh_available).toBe(true)
  expect(hosts.hosts).toHaveLength(3)
  expect(call).toHaveBeenCalledWith('cloud_ssh_hosts', [])

  const opts: SshOptions = { host: 'prod-box', user: 'ubuntu', port: 2222, identity: '~/.ssh/id_prod', name: null }
  const probe = await api.probe(opts)
  expect(probe).toEqual({ os: 'linux', arch: 'aarch64', ok: true, error: null })
  expect(call).toHaveBeenCalledWith('cloud_ssh_probe', [opts])

  // Node name suggestion & create payload shape (no size/region required for SSH).
  expect(suggestSshNodeName('Prod_Box.local', ['prod-box-local'])).toBe('prod-box-local-2')
  const spec: NodeSpec = {
    provider: 'ssh',
    name: 'prod-box',
    region: '',
    memory_gb: 0,
    volume_gb: null,
    k8s: null,
    ssh: { ...opts, name: 'prod-box' },
    desktop: false,
  }
  expect(spec.provider).toBe('ssh')
  expect(spec.ssh?.host).toBe('prod-box')
  expect(spec.ssh?.identity).toBe('~/.ssh/id_prod')
  expect(spec.desktop).toBe(false)
})

test('SSH stageRows, placeChip, NodeRow (no Resize), and DestroyBody (no droplet copy)', () => {
  const rows = stageRows({
    node: sshNode({ state: 'creating', stage: 'install' }),
    progress: { stage: 'install', pct: 55, eta_secs: 45, lines: ['Detected Linux x86_64', 'Uploading engine'] },
    version: '2.31.2',
  })
  expect(rows.map((r) => r.stage)).toEqual(['server', 'install', 'join', 'vault', 'ready'])
  expect(rows.map((r) => r.status)).toEqual(['done', 'active', 'waiting', 'waiting', 'waiting'])
  expect(rows[0]?.label).toBe('Detected Linux x86_64 on prod-box')
  expect(rows[1]?.label).toContain('Uploading and installing theuth 2.31.2 (signed)')

  const n = sshNode()
  expect(placeChip(n)).toBe('SSH · prod-box')

  const rowHtml = html(
    createElement(NodeRow, {
      node: n,
      onRestart: () => {},
      onUpdate: () => {},
      onTerminal: () => {},
      onDesktop: () => {},
      opening: false,
      onOpen: () => {},
    }),
  )
  expect(rowHtml).toContain('SSH · prod-box')
  expect(rowHtml).not.toContain('Resize')

  const keepHtml = html(createElement(AlertDialog, { open: true }, createElement(DestroyBody, { node: n, perGb: null, keep: true, onKeep: () => {} })))
  expect(keepHtml).toContain('Destroy prod-box?')
  expect(keepHtml).toContain('uninstalls the theuth service on prod-box')
  expect(keepHtml).toContain('Keep ~/.theuth data on prod-box')
  expect(keepHtml.toLowerCase()).not.toContain('droplet')

  const wipeHtml = html(createElement(AlertDialog, { open: true }, createElement(DestroyBody, { node: n, perGb: null, keep: false, onKeep: () => {} })))
  expect(wipeHtml).toContain('removes ~/.theuth')
  expect(wipeHtml.toLowerCase()).not.toContain('droplet')
})
