import type { DeviceInfo } from '@/rpc/bindings/method.nodes.output'

/** A device neither connected nor seen (or paired) for this long is listed under Inactive. */
export const INACTIVE_MS = 90 * 24 * 60 * 60 * 1000

/** Where a row of the mesh's device registry sits in Settings › Network › Devices. */
export interface DeviceGroups {
  /** Not revoked, connected now or active within {@link INACTIVE_MS}. */
  active: DeviceInfo[]
  /** Not revoked, not seen for {@link INACTIVE_MS} (never seen: counted from pairing). */
  inactive: DeviceInfo[]
  /** Refused by every node; the engine deletes them 30 days after the revoke. */
  revoked: DeviceInfo[]
}

export type DeviceStatus = 'connected' | 'offline' | 'revoked'

export function deviceStatus(d: DeviceInfo, online: ReadonlySet<string>): DeviceStatus {
  if (d.revoked) return 'revoked'
  return online.has(d.id) ? 'connected' : 'offline'
}

/** `online`: ids with a live session on this node (`link_status.devices[].online`). */
export function groupDevices(devices: readonly DeviceInfo[], online: ReadonlySet<string>, now = Date.now()): DeviceGroups {
  const groups: DeviceGroups = { active: [], inactive: [], revoked: [] }
  for (const d of devices) {
    if (d.revoked) groups.revoked.push(d)
    else if (!online.has(d.id) && now - (d.last_seen_ms || d.paired_ms) >= INACTIVE_MS) groups.inactive.push(d)
    else groups.active.push(d)
  }
  return groups
}

/**
 * Delete a device across the mesh. `device_revoke` on an active device revokes it (its connection closes now); on a
 * revoked one it deletes the row — so an active device takes two calls, a revoked one a single call.
 */
export async function deleteDevice(revoke: (id: string) => Promise<unknown>, d: DeviceInfo): Promise<void> {
  if (!d.revoked) await revoke(d.id)
  await revoke(d.id)
}

/** {@link deleteDevice} for each, one at a time (the engine serializes them anyway). */
export async function deleteDevices(revoke: (id: string) => Promise<unknown>, devices: readonly DeviceInfo[]): Promise<void> {
  for (const d of devices) await deleteDevice(revoke, d)
}
