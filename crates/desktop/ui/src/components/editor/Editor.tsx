import { withCollaboration } from '@blocknote/core/yjs'
import { filterSuggestionItems } from '@blocknote/core/extensions'
import { SuggestionMenuController, getDefaultReactSlashMenuItems, useCreateBlockNote } from '@blocknote/react'
import { BlockNoteView } from '@blocknote/shadcn'
import '@blocknote/shadcn/style.css'
import { FRAGMENT, createDocumentSchema } from '@theuth/documents/schema'
import { useEffect, useState, useSyncExternalStore } from 'react'
import { frontmatterBlock } from '@/components/editor/FrontmatterBlock'
import type { DocStatus, HarnessProvider } from '@/lib/doc/provider'
import { subscribeDark } from '@/lib/theme'
import { cn } from 'cn'

/** The schema of 59 §9.2 with the React frontmatter block; identical ProseMirror shape to the helper's. */
const schema = createDocumentSchema(frontmatterBlock)

/** The slash menu of 59 §9.3: the schema's blocks, headings 1–3 only, no toggle headings, no emoji picker. */
const SLASH_KEYS: Record<string, true> = {
  paragraph: true,
  heading: true,
  heading_2: true,
  heading_3: true,
  bullet_list: true,
  numbered_list: true,
  check_list: true,
  toggle_list: true,
  code_block: true,
  table: true,
  quote: true,
  divider: true,
}

const hhmm = (d: Date) => d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })

/** BlockNote over a live `HarnessProvider` (59 §3.3); the status line of §9.4 sits under it. */
export function Editor({ provider, status, className }: { provider: HarnessProvider; status: DocStatus; className?: string }) {
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'))
  const editor = useCreateBlockNote(
    withCollaboration({
      schema,
      collaboration: {
        provider,
        fragment: provider.doc.getXmlFragment(FRAGMENT),
        user: { name: 'you', color: '#b4531e' },
      },
    }),
    [provider],
  )
  const [exported, setExported] = useState<Date | null>(null)
  const [fromFile, setFromFile] = useState<Date | null>(null)
  useEffect(
    () =>
      provider.onEvent((e) => {
        if (e.type === 'Exported') {
          setExported(new Date())
          setFromFile(null)
        } else if (e.type === 'Update') setFromFile(new Date())
      }),
    [provider],
  )
  return (
    <div className={cn('flex min-h-0 flex-1 flex-col', className)}>
      <BlockNoteView
        editor={editor}
        theme={dark ? 'dark' : 'light'}
        editable={status.kind === 'live' || status.kind === 'offline'}
        slashMenu={false}
        className="min-h-0 flex-1 overflow-y-auto px-6 py-4 [&_.bn-editor]:mx-auto [&_.bn-editor]:max-w-[768px]"
      >
        <SuggestionMenuController
          triggerCharacter="/"
          getItems={async (query) =>
            filterSuggestionItems(
              getDefaultReactSlashMenuItems(editor).filter((item) => SLASH_KEYS[(item as { key?: string }).key ?? '']),
              query,
            )
          }
        />
      </BlockNoteView>
      <footer className="flex h-7 shrink-0 items-center gap-3 border-t border-border-subtle px-4 font-mono text-[11px] text-fg-tertiary">
        <span className={cn(status.kind === 'live' && 'text-fg-secondary', status.kind === 'closed' && 'text-danger')}>
          {status.kind === 'offline' ? `offline · ${status.queued} queued` : status.kind === 'closed' ? `closed · ${status.reason}` : status.kind === 'readonly' ? 'read only' : status.kind}
        </span>
        {fromFile && <span>updated from file {hhmm(fromFile)}</span>}
        {exported && !fromFile && <span>exported {hhmm(exported)}</span>}
      </footer>
    </div>
  )
}
