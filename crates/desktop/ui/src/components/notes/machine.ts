// 184 §12: machine names for the note chip, Move to ▸, the Share copy and the cells' "ran on" line.
import { engines } from '@/components/tasks/home'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'

export type Mesh = Pick<MeshStatus, 'self_id' | 'self_name' | 'peers'> | undefined

/** "In a mesh" = `mesh_status.peers` is not empty (a lone engine with a paired phone is not). */
export const inMesh = (mesh: Mesh): boolean => !!mesh && mesh.peers.length > 0

/** The machine's name; an id the mesh does not know shows its first 8 characters. */
export const machineName = (mesh: Mesh, id: string): string => engines(mesh).find((e) => e.id === id)?.name ?? id.slice(0, 8)
