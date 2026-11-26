import { ChevronRight, Pencil } from 'lucide-react'
import { useState } from 'react'
import { Dot, Empty, RenameInput, Row } from '@/components/settings/shared'
import { age } from '@/components/tasks/helpers'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import type { DeviceInfo } from '@/rpc/bindings/method.nodes.output'
import { type DeviceStatus, deviceStatus, groupDevices } from './devices'

const PLATFORM: Record<string, string> = { ios: 'iOS', macos: 'macOS', web: 'Web', theuth: 'theuth' }
const STATUS: Record<DeviceStatus, string> = { connected: 'connected now', offline: 'offline', revoked: 'revoked' }

/**
 * The mesh's device registry (69 §3), grouped: active devices, then Inactive (not seen for 90 days, with "Delete all
 * inactive"), then a collapsed Revoked group. Every row: name, platform, last seen, status; Revoke while not revoked,
 * Delete always.
 */
export function DeviceList({
  devices,
  online,
  now = Date.now(),
  onRename,
  onRevoke,
  onDelete,
  onDeleteAll,
}: {
  devices: readonly DeviceInfo[]
  /** Ids with a live session on this node. */
  online: ReadonlySet<string>
  now?: number
  onRename: (d: DeviceInfo, name: string) => void
  onRevoke: (d: DeviceInfo) => void
  onDelete: (d: DeviceInfo) => void
  onDeleteAll: (ds: DeviceInfo[]) => void
}) {
  const { active, inactive, revoked } = groupDevices(devices, online, now)
  const [showRevoked, setShowRevoked] = useState(false)
  const row = (d: DeviceInfo) => (
    <DeviceRow key={d.id} device={d} status={deviceStatus(d, online)} now={now} onRename={(name) => onRename(d, name)} onRevoke={() => onRevoke(d)} onDelete={() => onDelete(d)} />
  )
  return (
    <>
      {active.length === 0 && inactive.length === 0 && <Empty>No device paired with this mesh yet. A QR code is valid for five minutes and pairs one device.</Empty>}
      {active.map(row)}
      {inactive.length > 0 && (
        <div data-device-group="inactive" className="pt-4">
          <div className="flex items-center justify-between">
            <h4 className="text-[13px] font-medium text-fg-secondary">Inactive · not seen for 90 days ({inactive.length})</h4>
            <Confirm
              trigger="Delete all inactive"
              title={`Delete ${inactive.length} inactive device${inactive.length === 1 ? '' : 's'}?`}
              description="Each is revoked and removed from every node of the mesh. Using one again needs a new QR scan."
              onConfirm={() => onDeleteAll(inactive)}
            />
          </div>
          {inactive.map(row)}
        </div>
      )}
      {revoked.length > 0 && (
        <div data-device-group="revoked" className="pt-4">
          <button type="button" aria-expanded={showRevoked} className="flex items-center gap-1 text-[13px] font-medium text-fg-secondary" onClick={() => setShowRevoked((v) => !v)}>
            <ChevronRight className={`size-3.5 transition-transform ${showRevoked ? 'rotate-90' : ''}`} />
            Revoked ({revoked.length})
          </button>
          {showRevoked && (
            <>
              <p className="pt-1 text-[13px] text-fg-tertiary">Refused by every node; removed automatically 30 days after the revoke.</p>
              {revoked.map(row)}
            </>
          )}
        </div>
      )}
    </>
  )
}

function DeviceRow({
  device: d,
  status,
  now,
  onRename,
  onRevoke,
  onDelete,
}: {
  device: DeviceInfo
  status: DeviceStatus
  now: number
  onRename: (name: string) => void
  onRevoke: () => void
  onDelete: () => void
}) {
  const [editing, setEditing] = useState(false)
  const seen = status === 'connected' ? 'seen now' : d.last_seen_ms ? `seen ${age(d.last_seen_ms, now)} ago` : 'never seen'
  return (
    <Row
      label={
        <span className={`flex items-center gap-2 ${d.revoked ? 'text-fg-tertiary' : ''}`}>
          <Dot on={status === 'connected'} />
          {editing ? (
            <RenameInput value={d.name} label="Device name" onSave={onRename} onDone={() => setEditing(false)} />
          ) : (
            <>
              <span className="max-w-72 truncate text-sm">{d.name}</span>
              {!d.revoked && (
                <Button variant="ghost" size="icon-sm" aria-label="Rename" onClick={() => setEditing(true)}>
                  <Pencil />
                </Button>
              )}
            </>
          )}
        </span>
      }
      description={[PLATFORM[d.platform] ?? d.platform, seen, STATUS[status]].join(' · ')}
    >
      {!d.revoked && (
        <Confirm
          trigger="Revoke"
          title={`Revoke ${d.name}?`}
          description="Its connection closes now and every node of the mesh refuses it; pairing again needs a new QR scan."
          onConfirm={onRevoke}
        />
      )}
      <Confirm
        trigger="Delete"
        title={`Delete ${d.name}?`}
        description={`${d.revoked ? '' : 'It is revoked, then '}removed from every node of the mesh. Using it again needs a new QR scan.`}
        onConfirm={onDelete}
      />
    </Row>
  )
}

function Confirm({ trigger, title, description, onConfirm }: { trigger: string; title: string; description: string; onConfirm: () => void }) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="outline" size="sm">
          {trigger}
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>{trigger}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
