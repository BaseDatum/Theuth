import { SkillBody } from '@/components/library/Skills'
import type { SkillProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind } from '@/components/tiles/types'

/** A skill's markdown beside the chat that injected it (`SkillCard`'s sheet body); one per skill name. */
export const skillTile: TileKind = {
  title: (tile) => (tile.props as SkillProps).name,
  render: ({ tile, first, close }) => {
    const { name } = tile.props as SkillProps
    return (
      <TileFrame title={name} first={first} onClose={close} className="pt-4">
        <SkillBody name={name} />
      </TileFrame>
    )
  },
}
