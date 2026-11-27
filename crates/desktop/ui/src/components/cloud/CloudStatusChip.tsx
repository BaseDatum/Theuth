// 224: compact status-bar chip while any cloud node is `creating`, so leaving Settings or closing the wizard
// is never a dead end. One creating node opens its status dialog directly (`?progress=<name>` on desktop,
// `/cloud/new?node=<name>` on phone); several open Settings › Machines (each row has "View status").

import { useNavigate } from '@tanstack/react-router'
import { LoaderCircle } from 'lucide-react'
import { useSupported } from '@/rpc/capabilities'
import { CLOUD_REQUIRED, useCloudList } from './queries'

/** Guarded by `CLOUD_REQUIRED`, so engines without `cloudnodes` (and fixtures/tests without discovery) render nothing. */
export function CloudCreatingChip({ mobile = false, onOpen }: { mobile?: boolean; onOpen?: () => void }) {
  const supported = useSupported(CLOUD_REQUIRED)
  if (!supported) return null
  return <CloudCreatingChipInner mobile={mobile} onOpen={onOpen} />
}

export function CloudCreatingChipInner({ mobile = false, onOpen }: { mobile?: boolean; onOpen?: () => void }) {
  const navigate = useNavigate()
  const list = useCloudList()
  const creating = (list.data?.nodes ?? []).filter((n) => n.state === 'creating')
  if (creating.length === 0) return null

  if (mobile && creating.length > 1) {
    return (
      <div data-cloud-creating={creating.length} className="flex flex-wrap items-center gap-2">
        {creating.map((n) => (
          <button
            key={n.name}
            type="button"
            className="flex shrink-0 items-center gap-1 text-xs text-fg-secondary hover:text-foreground hover:underline"
            onClick={() => {
              onOpen?.()
              void navigate({ to: '/cloud/new', search: { entry: 'add-to-mesh', node: n.name } })
            }}
          >
            <LoaderCircle aria-hidden className="size-2.5 animate-spin text-primary motion-reduce:animate-none" />
            <span>Starting {n.name}</span>
          </button>
        ))}
      </div>
    )
  }

  const single = creating.length === 1 ? creating[0]! : null
  const label = single ? `Starting ${single.name}` : `${creating.length} cloud nodes starting`
  const title = single
    ? `View provisioning status for ${single.name}`
    : `${creating.length} cloud nodes starting — open Settings › Machines`

  const open = () => {
    onOpen?.()
    if (single) {
      if (mobile) void navigate({ to: '/cloud/new', search: { entry: 'add-to-mesh', node: single.name } })
      else void navigate({ to: '/settings/$tab', params: { tab: 'machines' }, search: { progress: single.name } })
    } else {
      void navigate({ to: '/settings/$tab', params: { tab: 'machines' } })
    }
  }

  return (
    <button
      type="button"
      data-cloud-creating={creating.length}
      title={title}
      className="flex shrink-0 items-center gap-1 text-fg-secondary hover:text-foreground hover:underline"
      onClick={open}
    >
      <LoaderCircle aria-hidden className="size-2.5 animate-spin text-primary motion-reduce:animate-none" />
      <span>{label}</span>
    </button>
  )
}
