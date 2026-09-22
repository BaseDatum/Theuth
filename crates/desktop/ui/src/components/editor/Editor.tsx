import { withCollaboration } from '@blocknote/core/yjs'
import { SideMenuExtension, filterSuggestionItems } from '@blocknote/core/extensions'
import {
  SideMenu,
  SideMenuController,
  type SideMenuProps,
  SuggestionMenuController,
  getDefaultReactSlashMenuItems,
  useCreateBlockNote,
  useExtensionState,
} from '@blocknote/react'
import { BlockNoteView } from '@blocknote/shadcn'
import '@blocknote/shadcn/style.css'
import { FRAGMENT, createDocumentSchema } from '@theuth/documents/schema'
import { Sparkles } from 'lucide-react'
import { useEffect, useMemo, useState, useSyncExternalStore } from 'react'
import { frontmatterBlock } from '@/components/editor/FrontmatterBlock'
import { MobileToolbar } from '@/components/editor/MobileToolbar'
import { revealExtension } from '@/components/editor/reveal'
import type { AgentPresence, DocStatus, HarnessProvider } from '@/lib/doc/provider'
import { usePointerCoarse } from '@/lib/mobile'
import { deviceName } from '@/lib/platform'
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

/** Remote-cursor palette for owner devices (59 §8.5); the agent's colour comes from `meta.agent.color`. */
const CURSOR_COLORS = ['#b4531e', '#1e6fb4', '#2e8b57', '#b41e7a', '#8a6d1e', '#1e8a8a']

const hhmm = (d: Date) => d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })

/** The default side menu, except on the frontmatter block: no drag handle, no block menu, no "+" (59 §4.5). */
function DocSideMenu(props: SideMenuProps) {
  const type = useExtensionState(SideMenuExtension, { selector: (s) => s?.block?.type })
  return type === 'frontmatter' ? null : <SideMenu {...props} />
}

/**
 * The page frame (59 §24.y, §24.8): the scroll container is the canvas, `.bn-editor` the paper (styles.css `html .bn-editor`:
 * BlockNote's own sheet is unlayered, so utilities on `.bn-editor` would lose). The canvas is a column flex whose one
 * growing item is the paper, so an empty document is still page-high and the whole page is the editable element.
 * `dense` (the meeting pane, already in a bordered box) keeps the paper but tightens it.
 */
const CANVAS = 'min-h-0 flex-1 overflow-y-auto bg-background'

/**
 * BlockNote over a live `HarnessProvider` (59 §3.3); the status line of §9.4 sits under it.
 * `onAsk(block)` is the "Ask theuth…" slash item (§9.3): the host opens a chat about this document and block.
 */
export function Editor({
  provider,
  status,
  className,
  onAsk,
  dense,
}: {
  provider: HarnessProvider
  status: DocStatus
  className?: string
  onAsk?: (block: string) => void
  dense?: boolean
}) {
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'))
  // 59 §10: fingers get the fixed toolbar instead of BlockNote's floating menus; labels on remote cursors need no hover.
  const coarse = usePointerCoarse()
  const [toolbar, setToolbar] = useState(false)
  const editable = status.kind === 'live' || status.kind === 'offline'
  const agentColor = provider.meta?.agent.color ?? '#7c3aed'
  const editor = useCreateBlockNote(
    withCollaboration({
      schema,
      collaboration: {
        provider,
        fragment: provider.doc.getXmlFragment(FRAGMENT),
        // 59 §8.5: no owner-name setting exists yet; the device label (`Mac`, `iPad`, `Chrome`) tells cursors
        // apart, the colour is picked by Yjs client id so two devices rarely share one.
        user: { name: deviceName(), color: CURSOR_COLORS[provider.doc.clientID % CURSOR_COLORS.length] as string },
        showCursorLabels: coarse ? 'always' : 'activity',
      },
      extensions: [revealExtension(provider, agentColor)],
    }),
    [provider, agentColor, coarse],
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
  const agent = useAgentPresence(provider)
  const askItem = useMemo(
    () =>
      onAsk
        ? [
            {
              key: 'ask_theuth',
              title: 'Ask theuth…',
              subtext: 'Chat about this block',
              group: 'theuth',
              aliases: ['ask', 'theuth', 'ai'],
              icon: <Sparkles className="size-4" />,
              onItemClick: () => onAsk(editor.getTextCursorPosition().block.id),
            },
          ]
        : [],
    [onAsk, editor],
  )
  return (
    <div className={cn('flex min-h-0 flex-1 flex-col', className)} data-toolbar={toolbar || undefined} data-paper={dense ? 'dense' : 'page'}>
      <BlockNoteView
        editor={editor}
        theme={dark ? 'dark' : 'light'}
        editable={editable}
        slashMenu={false}
        sideMenu={false}
        formattingToolbar={!coarse}
        linkToolbar={!coarse}
        tableHandles={!coarse}
        className={cn(CANVAS, dense ? 'p-3 md:p-4' : 'md:p-6')}
      >
        {coarse ? (
          <MobileToolbar editor={editor} readonly={!editable} onVisible={setToolbar} />
        ) : (
          <>
            <SuggestionMenuController
              triggerCharacter="/"
              getItems={async (query) =>
                filterSuggestionItems(
                  [...getDefaultReactSlashMenuItems(editor).filter((item) => SLASH_KEYS[(item as { key?: string }).key ?? '']), ...askItem],
                  query,
                )
              }
            />
            <SideMenuController sideMenu={DocSideMenu} />
          </>
        )}
      </BlockNoteView>
      <footer className="flex h-7 shrink-0 items-center gap-3 border-t border-border-subtle px-4 font-mono text-[11px] text-fg-tertiary">
        <span className={cn(status.kind === 'live' && 'text-fg-secondary', status.kind === 'closed' && 'text-danger')}>
          {status.kind === 'offline' ? `offline · ${status.queued} queued` : status.kind === 'closed' ? `closed · ${status.reason}` : status.kind === 'readonly' ? 'read only' : status.kind}
        </span>
        {fromFile && <span>updated from file {hhmm(fromFile)}</span>}
        {exported && !fromFile && <span>exported {hhmm(exported)}</span>}
        {agent && (
          <span
            className="ml-auto rounded px-1.5 text-white"
            style={{ background: agent.color }}
            title={agent.blocks.length > 0 ? `editing ${agent.blocks.length} block${agent.blocks.length === 1 ? '' : 's'}` : 'editing'}
          >
            {agent.name}
          </span>
        )}
      </footer>
    </div>
  )
}

/** The agent's awareness state (59 §6.4, §9.4), re-read on every awareness change. */
function useAgentPresence(provider: HarnessProvider): AgentPresence | null {
  const [agent, setAgent] = useState<AgentPresence | null>(() => provider.agentPresence())
  useEffect(() => {
    const read = () => setAgent(provider.agentPresence())
    read()
    provider.awareness.on('change', read)
    return () => provider.awareness.off('change', read)
  }, [provider])
  return agent
}
