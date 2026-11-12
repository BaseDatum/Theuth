import { useQuerySupport } from '@/rpc/capabilities'
// Owner: U2
import { useQuery } from '@tanstack/react-query'
import type { Interpreter } from "@/rpc/bindings/method.cells.output"
import type { NoteDefaults } from '@/rpc/bindings/auxiliary.surface.output'
import { useConnection } from '@/store/connection'
import { parseDefaults } from './grammar'
import './note-cells.css'

export type CellDefaultsProps = {
  /** The ```` ```cells ```` block's text. */
  text: string
  // 171:U2-ADD — a later `cells` block (§3.3: ignored, the engine's warning); a click reveals the block's text.
  second?: boolean
  onOpen?: () => void
}

/** `bash 5.2 (Homebrew)` — the kernel chip's words (§6.3) for the note's `shell:`. */
export function shellLabel(i: Interpreter): string {
  const version = i.version?.split('.').slice(0, 2).join('.')
  const source = i.source === 'homebrew' ? 'Homebrew' : i.source
  return `${i.kind}${version ? ` ${version}` : ''}${source ? ` (${source})` : ''}`
}

/** "Cell defaults · ~/dr-t6/frp-integration · bash 5.2 (Homebrew) · .venv": the parsed keys in order, the shell as named by `cell_interpreters`. */
export function defaultsLine(d: NoteDefaults, shell: Interpreter | undefined): string {
  const python = d.python && (d.python.kind === 'path' ? d.python.value : d.python.kind === 'venv' ? '.venv' : d.python.kind)
  return ['Cell defaults', d.cwd, d.shell && (shell ? shellLabel(shell) : d.shell.split('/').at(-1)), python].filter(Boolean).join(' · ')
}

/**
 * 171 §3.3, §11.5: the note's `cells` defaults block as one muted line; clicking it reveals the block's text for
 * editing in place (the host parks BlockNote's content while the line shows). A second `cells` block shows the
 * engine's lint text instead. The shell's label needs the owner's engine (`cell_interpreters`); elsewhere the path's
 * last segment stands in.
 */
export function CellDefaults({ text, second = false, onOpen }: CellDefaultsProps) {
  const canQuery1 = useQuerySupport(["cell_interpreters"])
  const api = useConnection((s) => s.api)
  const d = parseDefaults(text)
  const q = useQuery({
    queryKey: ['cells', 'interpreters', d.cwd ?? '~'],
    queryFn: () => (api ? api.cell_interpreters(d.cwd ?? '~', null) : Promise.resolve([])),
    enabled: canQuery1 && (!!api && !!d.shell && !second),
    staleTime: 60_000,
  })
  const shell = q.data?.find((i) => i.path === d.shell)
  return (
    <button
      type="button"
      contentEditable={false}
      data-note-cell=""
      data-defaults=""
      className="block w-full truncate rounded px-1 py-0.5 text-left text-xs text-fg-tertiary hover:bg-layer-2 pointer-coarse:min-h-12"
      title="Edit cell defaults"
      onClick={onOpen}
    >
      {second ? 'a second `cells` block is ignored' : defaultsLine(d, shell)}
    </button>
  )
}
