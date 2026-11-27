// TEMP mirror of crates/cloudnodes/src/types.rs until bindings regenerate
// (design 224 §13). Money is USD per month, list prices.

export type ProviderKind = 'digitalocean' | 'fly' | 'kubernetes' | 'lightsail'

export const SIZES_GB = [2, 4, 8] as const
export const DEFAULT_VOLUME_GB = 10

export type SizeOption = { memory_gb: number; vcpus: number; slug: string; monthly_usd: number | null }

export type Region = { id: string; label: string; country: string }

export type Discovered = { id: string; label: string; detail: string | null; path: string }

export type CredentialSource = 'oauth' | 'pasted' | 'imported' | 'kubeconfig'

export type ProviderInfo = {
  provider: ProviderKind
  label: string
  available: boolean
  beta: boolean
  desktop_only: boolean
  connected: boolean
  source: CredentialSource | null
  discovered: Discovered[]
  oauth: boolean
  sizes: SizeOption[]
  regions: Region[]
  volume_usd_per_gb: number | null
}

export type K8sOptions = {
  context: string | null
  namespace: string | null
  create_namespace?: boolean
  storage_class: string | null
  deny_cluster_network?: boolean
}

export type NodeSpec = {
  provider: ProviderKind
  name: string
  region: string
  memory_gb: number
  volume_gb: number | null
  k8s: K8sOptions | null
}

export type Quote = {
  provider: ProviderKind
  size_slug: string
  server_usd: number | null
  disk_usd: number | null
  total_usd: number | null
}

export type Stage = 'server' | 'storage' | 'install' | 'join' | 'ready'
export const STAGES: readonly Stage[] = ['server', 'storage', 'install', 'join', 'ready']

export type NodeState = 'creating' | 'ready' | 'failed' | 'busy' | 'destroying' | 'orphan' | 'volume_kept'

export type MeshPath = 'direct' | 'relay' | 'offline'

export type CloudNode = {
  name: string
  provider: ProviderKind
  region: string | null
  region_label: string | null
  memory_gb: number | null
  size_slug: string | null
  volume_gb: number | null
  monthly_usd: number | null
  state: NodeState
  stage: Stage | null
  error: string | null
  provider_state: string | null
  ip: string | null
  peer_id: string | null
  reachable: boolean | null
  path: MeshPath | null
  version: string | null
  auto_update: boolean
  created_ms: number | null
  k8s: K8sOptions | null
}

export type ProviderError = { provider: ProviderKind; message: string }

export type CloudList = { nodes: CloudNode[]; errors: ProviderError[] }

export type StorageClass = { name: string; provisioner: string; default: boolean }

export type K8sInfo = {
  contexts: string[]
  current_context: string | null
  namespaces: string[]
  storage_classes: StorageClass[]
  no_default_class: boolean
  cni: string | null
  enforces_network_policy: boolean | null
}

export type OauthStart = { url: string }

export type TerminalOpen = { session: string; job: string; engine: string }

export type NodeLogs = { name: string; lines: string[] }

export type CloudEvent =
  | { type: 'CloudNodeChanged'; node: CloudNode }
  | { type: 'CloudNodeProgress'; name: string; stage: Stage | null; line: string; pct: number | null; eta_secs: number | null }
  | { type: 'CloudNodeRemoved'; name: string }
  | { type: 'CloudCredentialChanged'; provider: ProviderKind; connected: boolean; error: string | null }

/** The `cloud_*` methods as `Api` will carry them once the bindings regenerate. */
export type CloudApi = {
  cloud_providers(): Promise<ProviderInfo[]>
  cloud_import_credential(provider: ProviderKind, id: string): Promise<ProviderInfo>
  cloud_set_token(provider: ProviderKind, token: string): Promise<ProviderInfo>
  cloud_forget_credential(provider: ProviderKind): Promise<null>
  cloud_oauth_start(provider: ProviderKind): Promise<OauthStart>
  cloud_oauth_cancel(provider: ProviderKind): Promise<null>
  cloud_quote(spec: NodeSpec): Promise<Quote>
  cloud_k8s_info(context: string | null): Promise<K8sInfo>
  cloud_create(spec: NodeSpec): Promise<CloudNode>
  cloud_list(): Promise<CloudList>
  cloud_status(name: string): Promise<CloudNode>
  cloud_restart(name: string): Promise<unknown>
  cloud_resize(name: string, memory_gb: number): Promise<unknown>
  cloud_update(name: string): Promise<unknown>
  cloud_destroy(name: string, keep_volume: boolean): Promise<string[]>
  cloud_terminal(name: string): Promise<TerminalOpen>
  cloud_logs(name: string, tail: number | null): Promise<NodeLogs>
}
