import { save } from '@tauri-apps/plugin-dialog'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import { useNavigate } from '@tanstack/react-router'
import { Download } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { diagramTokens, lightTokens } from '@/lib/diagram-theme'
import { exportAssets, mountedDiagrams } from '@/lib/doc-export'
import { isLocal } from '@/lib/files'
import { useApi } from '@/lib/rpc'
import { useEngines } from '@/link/engines'
import type { ExportFormat } from '@/rpc/bindings/ExportFormat'

const EXT: Record<ExportFormat, { ext: string; name: string }> = { markdown: { ext: 'md', name: 'Markdown' }, pdf: { ext: 'pdf', name: 'PDF' } }

/**
 * Copies `markdown()` as plain text. WebKit (WKWebView, iOS, Safari) refuses a clipboard write begun after an
 * `await` — the gesture has expired — so the write starts now with the text promised inside a `ClipboardItem`.
 * `writeText` after the fetch where `ClipboardItem`/`clipboard.write` is missing or `write` fails other than
 * NotAllowed. A failed fetch rejects with its own error.
 */
export function copyMarkdown(markdown: () => Promise<string>): Promise<void> {
  const text = markdown()
  const { clipboard } = navigator
  if (typeof ClipboardItem === 'undefined' || !clipboard.write) return text.then((t) => clipboard.writeText(t))
  return clipboard.write([new ClipboardItem({ 'text/plain': text.then((t) => new Blob([t], { type: 'text/plain' })) })]).catch((e: unknown) =>
    text.then((t) => {
      if (e instanceof DOMException && e.name === 'NotAllowedError') throw e
      return clipboard.writeText(t)
    }),
  )
}

/**
 * 72 §3: **Export ▸ Markdown… / PDF… / Copy Markdown** in the note bar. Desktop on the local daemon: a save dialog, then
 * `doc_export` writes the file and the toast reveals it. Elsewhere (web, iOS, a remote engine): `doc_export` stores an
 * artifact and the viewer opens on it. PDF takes the mounted diagrams along as light-palette svgs (`lib/doc-export`).
 * Copy Markdown puts `doc_markdown` (what the Markdown export writes) on the clipboard, everywhere alike.
 */
export function ExportMenu({ id, title, disabled }: { id: string; title: string; disabled?: boolean }) {
  const api = useApi()
  const navigate = useNavigate()
  const local = isLocal(useEngines((s) => s.active))
  const run = async (format: ExportFormat) => {
    const { ext, name } = EXT[format]
    let dest: string | null = null
    if (local) {
      dest = await save({ defaultPath: `${title || 'note'}.${ext}`, filters: [{ name, extensions: [ext] }] })
      if (!dest) return
    }
    const assets = format === 'pdf' ? exportAssets(mountedDiagrams(document), diagramTokens(), lightTokens()) : []
    const r = await api.doc_export(id, format, dest, assets)
    for (const w of r.warnings) toast.warning(w)
    if (r.path) {
      const path = r.path
      toast.success(`Saved ${path.split('/').pop()}`, { action: { label: 'Reveal', onClick: () => void revealItemInDir(path) } })
    } else if (r.call_id) {
      void navigate({ to: '/artifacts/$callId', params: { callId: r.call_id } })
    }
  }
  const toastError = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))
  const pick = (format: ExportFormat) => run(format).catch(toastError)
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" className="size-11 rounded-xl" aria-label="Export" disabled={disabled}>
              <Download className="size-[18px]" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent>Export</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-44">
        <DropdownMenuItem onSelect={() => void pick('markdown')}>Markdown…</DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void pick('pdf')}>PDF…</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void copyMarkdown(() => api.doc_markdown(id)).then(() => toast.message('Copied Markdown'), toastError)}>Copy Markdown</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
