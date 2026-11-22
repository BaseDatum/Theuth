import type { VmProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { VmDesktop } from '@/components/vm/VmDesktop'
import { requirements } from '@/components/vm/requirements'
import { openPopout } from '@/lib/popout'
import { Capability } from '@/rpc/capabilities'

/** A sandbox VM's live desktop beside the chat (what `/vm/$name` shows full window). Open in new window moves the
 * desktop there — the tile closes once the window opened (a refusal leaves it), so one RFB session per VM is on screen. */
function VmTile({ tile, first, close }: TileRender) {
  const { name } = tile.props as VmProps
  const popOut = () => void openPopout({ kind: 'vm', name }).then(close, (e: unknown) => console.warn('open_window', e))
  return (
    <TileFrame first={first} onClose={close} title={name} popOut={popOut}>
      <Capability required={requirements}>
        <VmDesktop key={name} name={name} />
      </Capability>
    </TileFrame>
  )
}

export const vmTile: TileKind = {
  title: (tile) => (tile.props as VmProps).name,
  render: (p) => <VmTile {...p} />,
}
