import {
  FocusWindowButton,
  Terminal,
  TerminalScreen,
  TerminalSelect,
  usePoppedElsewhere,
  useTerminals,
} from '@/components/chat/TerminalViewer'
import type { JobRef, JobTerminalProps, TerminalsProps } from '@/components/tiles/open'
import { TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { openPopout } from '@/lib/popout'
import { setTileProps } from '@/lib/tiles'

/** The session's terminals (`TerminalViewer`'s dialog) beside the chat; the picker and pop-out sit in the header. */
function Terminals({ tile, first, close }: TileRender) {
  const { job: picked } = tile.props as TerminalsProps
  const terminals = useTerminals(tile.session)
  const head = terminals[0]
  const job = picked ?? (head ? { session: head.session, job: head.id } : null)
  const elsewhere = usePoppedElsewhere(job)
  // Trellis remembers the pick (the view's params), so a restored layout shows the same terminal.
  const pick = (j: JobRef) => setTileProps(tile.id, { job: j } satisfies TerminalsProps)
  return (
    <TileFrame
      first={first}
      onClose={close}
      title="Terminals"
      chat={tile.session}
      controls={
        <>
          <TerminalSelect session={tile.session} terminals={terminals} job={job} onPick={pick} className="w-48 max-w-[45%] shrink" />
          {job && elsewhere && <FocusWindowButton {...job} />}
        </>
      }
      popOut={job ? () => openPopout({ kind: 'pty', ...job }) : undefined}
      className="gap-2 p-2"
    >
      <TerminalScreen terminals={terminals} job={job} focus={false} />
    </TileFrame>
  )
}

export const terminalsTile: TileKind = {
  title: () => 'Terminals',
  render: (p) => <Terminals {...p} />,
}

export const jobTerminalTile: TileKind = {
  title: (tile) => `Terminal · ${(tile.props as JobTerminalProps).job}`,
  render: ({ tile, first, close }) => {
    const { session, job, label } = tile.props as JobTerminalProps
    return (
      <TileFrame
        first={first}
        onClose={close}
        title={`Terminal · ${job}`}
        tab={label || undefined}
        meta={label}
        popOut={() => openPopout({ kind: 'pty', session, job })}
        className="gap-2 p-2"
      >
        <Terminal session={session} job={job} label={label} />
      </TileFrame>
    )
  },
}
