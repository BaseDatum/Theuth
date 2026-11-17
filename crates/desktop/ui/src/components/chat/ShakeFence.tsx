// 197 §8: what a reply's surface fences need for their removal controls (drawn by `shakeable` in `MessageAssistant`):
// the row's pieces matched to the fences in its text, and the `Removed from context` marker with `View original`. The
// control exists only when the stored row's `fences` count equals the fences `surfaceFences` finds in the reply (the
// engine's rule): any disagreement means a fence index could name the wrong diagram, so there is none.
import { Trash2 } from 'lucide-react'
import { createContext, type ReactNode } from 'react'
import type { ContextPiece } from '@/rpc/bindings/method.agent.output'
import { type SurfaceFence, surfaceFences } from '@/lib/surface-fences'
import { useScopedDisclosure } from './disclosure'

/** What a reply offers its fences: the row's pieces beside the fences found in its text, counts already equal. */
export type ShakeFences = { session: string; active: boolean; pieces: ContextPiece[]; found: SurfaceFence[] }

export const ShakeFenceContext = createContext<ShakeFences | null>(null)

/** The reply's fence context, or `null` (no stored fences, or a count mismatch: no controls). */
export function shakeFences(session: string, active: boolean, text: string, pieces: ContextPiece[] | undefined): ShakeFences | null {
  if (!pieces?.length) return null
  const found = surfaceFences(text)
  return found.length === pieces.length ? { session, active, pieces, found } : null
}

/** The piece for the fence whose body is `code`; a body two fences share is ambiguous and gets none. Line ends and trailing
 * whitespace are not compared: the scanner keeps `\r\n` and the final line break, streamdown's `code` may not. */
export function pieceOf(ctx: ShakeFences, language: string, code: string): ContextPiece | null {
  const norm = (s: string) => s.replace(/\r\n/g, '\n').trimEnd()
  const body = norm(code)
  const at = ctx.found.flatMap((f, i) => (f.language === language && norm(f.body) === body ? [i] : []))
  return at.length === 1 ? (ctx.pieces[at[0]!] ?? null) : null
}

/** The collapsed marker (48 §2.2's tombstone, one fence wide); open state in the store under the reply's scope. */
export function RemovedFence({ piece, index, inner }: { piece: ContextPiece; index: number; inner: ReactNode }) {
  const [open, setOpen] = useScopedDisclosure(`removed-fence|${index}`)
  return (
    <div className="my-2 flex flex-col text-xs text-fg-tertiary">
      <div className="flex items-center gap-2 rounded-lg border border-dashed px-3">
        <Trash2 className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate">
          <span className="italic">Removed from context</span> · {piece.label}
        </span>
        <button type="button" className="min-h-11 shrink-0 px-2 underline underline-offset-2 hover:text-fg-secondary" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
          {open ? 'Hide original' : 'View original'}
        </button>
      </div>
      {open && (
        <div className="opacity-70" aria-label="Archived original; not part of the model's context">
          {inner}
        </div>
      )}
    </div>
  )
}
