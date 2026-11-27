import { useContext } from 'react'
import {
  FocusWindowButton,
  Terminal,
  TerminalScreen,
  TerminalSelect,
  usePoppedElsewhere,
  useTerminals,
} from '@/components/chat/TerminalViewer'
import type { JobRef, JobTerminalProps, TerminalsProps } from '@/components/tiles/open'
import { TileAlone, TileFrame } from '@/components/tiles/TileFrame'
import type { TileKind, TileRender } from '@/components/tiles/types'
import { openPopout } from '@/lib/popout'
import { setTileProps } from '@/lib/tiles'

/** The session's terminals (`TerminalViewer`'s dialog) beside the chat; the picker and pop-out sit in the header. */
function Terminals({ tile, first, close }: TileRender) {
  const { job: picked } = tile.props as TerminalsProps
  const terminals = useTerminals(tile.session)
  const head = terminals[0]
  const alone = useContext(TileAlone)
  const job = picked ?? (head ? { session: head.session, job: head.id } : null)
  const elsewhere = usePoppedElsewhere(alone ? null : job)
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
      <TerminalScreen terminals={terminals} job={job} focus={false} checkPopout={!alone} />
    </TileFrame>
  )
}

export const terminalsTile: TileKind = {
  title: () => 'Terminals',
  render: (p) => <Terminals {...p} />,
}

function JobTerminal({ tile, first, close }: TileRender) {
  const { session, job, label } = tile.props as JobTerminalProps
  const alone = useContext(TileAlone)
  const elsewhere = usePoppedElsewhere(alone ? null : { session, job })
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
      {elsewhere ? (
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 rounded-md p-4 text-sm" style={{ backgroundColor: '#12100e', color: '#e8dcc8' }}>
          <p>{label} · opened in its own window</p>
          <FocusWindowButton session={session} job={job} />
        </div>
      ) : (
        <Terminal session={session} job={job} label={label} />
      )}
    </TileFrame>
  )
}

export const jobTerminalTile: TileKind = {
  title: (tile) => `Terminal · ${(tile.props as JobTerminalProps).job}`,
  render: (p) => <JobTerminal {...p} />,
}
