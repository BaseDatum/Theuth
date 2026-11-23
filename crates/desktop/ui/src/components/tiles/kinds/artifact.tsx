import { ArtifactContent, dismissArtifact } from '@/components/chat/ArtifactPanel'
import type { ArtifactProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind } from '@/components/tiles/types'
import { useSessions } from '@/store/sessions'

/** The artifact sheet as a tile (35 §7.2): the same content, its header controls in the tile's header and Open in new
 * window (`/artifacts/$callId`, every artifact kind: image, svg, markdown, pdf, csv, html, …) in every tile's slot. Closing it
 * suppresses auto-open for the rest of the turn, as closing the sheet does. */
export const artifactTile: TileKind = {
  title(tile) {
    const { callId } = tile.props as ArtifactProps
    const s = useSessions.getState().sessions[tile.session]
    return s?.artifacts.get(callId)?.title ?? s?.appDrafts.get(callId)?.title ?? 'Artifact'
  },
  render({ tile, first, close }) {
    const { callId } = tile.props as ArtifactProps
    const dismiss = () => {
      dismissArtifact(tile.session)
      close()
    }
    // Keyed by the call: a retarget starts over (pinned version, lineage), as the sheet's `key={callId}` does.
    return (
      <ArtifactContent key={callId} session={tile.session} callId={callId} popOutApart>
        {(parts) => (
          <TileFrame
            first={first}
            onClose={dismiss}
            title={parts?.title ?? 'Artifact'}
            meta={parts?.meta}
            controls={parts?.controls}
            popOut={parts?.popOut}
          >
            {parts?.body}
          </TileFrame>
        )}
      </ArtifactContent>
    )
  },
}
