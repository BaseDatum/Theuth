// The cloud-node UI's names for the generated `cloudnodes` wire types (`crates/cloudnodes/src/types.rs`, design 224 §13),
// plus the few constants the wire does not carry.

import type { Api } from '@/rpc/bindings/client'
import type { CloudEvent } from '@/rpc/bindings/event.cloudnodes_2eevents.output'
import type * as I from '@/rpc/bindings/method.cloudnodes.input'
import type * as O from '@/rpc/bindings/method.cloudnodes.output'

export type {
  CloudList,
  CredentialSource,
  Discovered,
  MeshPath,
  NodeLogs,
  NodeState,
  OauthStart,
  ProviderError,
  ProviderInfo,
  ProviderKind,
  Quote,
  Region,
  SizeList,
  SizeOption,
  SshHostEntry,
  SshHosts,
  SshOptions,
  SshProbe,
  Stage,
  StorageClass,
  TerminalOpen,
  VaultProtection,
} from '@/rpc/bindings/method.cloudnodes.output'
export type { CloudEvent }

export type K8sOptions = O.K8SOptions
export type K8sInfo = O.K8SInfo
export type NodeDesktop = O.DesktopOpen

/** What the wizard sends: the wire's spec, with the desktop choice always stated. */
export type NodeSpec = I.NodeSpec & { desktop: boolean }

export type CloudNode = O.CloudNode & {
  /**
   * The node's vault is locked (sealed) until a device or the master password unlocks it. Comes from
   * feat/sealed-vault's engine; absent on engines without it.
   */
  sealed?: boolean
}

/** The `cloud_*` slice of `Api` (the fixture answers exactly these). */
export type CloudApi = Pick<Api, Extract<keyof Api, `cloud_${string}`>>

/** The 2 GB minimum and the two steps up (`types.rs` `SIZES_GB`). */
export const SIZES_GB = [2, 4, 8] as const
/** The `/data` volume every node gets (`types.rs` `DEFAULT_VOLUME_GB`). */
export const DEFAULT_VOLUME_GB = 10
/** The create job's stages, in order (`types.rs` `Stage`). */
export const STAGES: readonly O.Stage[] = ['server', 'storage', 'install', 'join', 'vault', 'ready']
