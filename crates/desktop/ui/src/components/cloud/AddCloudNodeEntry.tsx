// 224: the engine switcher's "Add a cloud node" card (phone; mockup §3): a dashed card that opens the wizard's
// add-to-mesh entry at `/cloud/new`. `onClick` replaces the navigation (a host that opens the wizard in place).

import { useNavigate } from '@tanstack/react-router'
import { Capability } from '@/rpc/capabilities'
import { CLOUD_REQUIRED } from './queries'
import { Card, ProviderBadge } from './steps'

export function AddCloudNodeEntry({ onClick }: { onClick?: () => void }) {
  const navigate = useNavigate()
  return (
    <Capability required={CLOUD_REQUIRED} hide>
      <button type="button" className="w-full rounded-xl text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50" onClick={onClick ?? (() => void navigate({ to: '/cloud/new', search: { entry: 'add-to-mesh' } }))}>
        <Card dashed className="flex items-center gap-2.5">
          <ProviderBadge kind="digitalocean" mark="＋" />
          <div className="min-w-0">
            <div className="text-sm font-semibold">Add a cloud node</div>
            <div className="text-xs text-fg-secondary">Always on, joins this mesh</div>
          </div>
        </Card>
      </button>
    </Capability>
  )
}
