import { withCollaboration } from '@blocknote/core/yjs'
import { SideMenuExtension, SyntaxHighlightingExtension, filterSuggestionItems, insertOrUpdateBlockForSlashMenu } from '@blocknote/core/extensions'
import {
  AddBlockButton,
  BlockColorsItem,
  DragHandleMenu,
  RemoveBlockItem,
  SideMenu,
  SideMenuController,
  type SideMenuProps,
  SuggestionMenuController,
  TableColumnHeaderItem,
  TableRowHeaderItem,
  getDefaultReactSlashMenuItems,
  useBlockNoteEditor,
  useComponentsContext,
  useCreateBlockNote,
  useDictionary,
  useExtension,
  useExtensionState,
} from '@blocknote/react'
import { BlockNoteView } from '@blocknote/shadcn'
import '@blocknote/shadcn/style.css'
import { FRAGMENT, createDocumentSchema, links } from '@theuth/documents/schema'
import { GripVertical, Sparkles, SquareTerminal, Workflow } from 'lucide-react'
import { type SyntheticEvent, useEffect, useMemo, useState, useSyncExternalStore } from 'react'
import { cellOf } from '@/components/cells/grammar'
import { NoteHost } from '@/components/cells/note-context'
import { CELL_STARTER, makeCell, makePlain, runnableLang } from '@/components/cells/note-edit'
import { useOpenCite, usePulseBlock } from '@/components/chat/Citations'
import { codeBlock } from '@/components/editor/CodeBlock'
import { CODE_BLOCK_OPTIONS } from '@/components/editor/codeLanguages'
import { frontmatterBlock } from '@/components/editor/FrontmatterBlock'
import { MobileToolbar } from '@/components/editor/MobileToolbar'
import { revealExtension } from '@/components/editor/reveal'
import { DictationButton } from '@/components/ui/dictation'
import { parseTheuthHref } from '@/lib/citations'
import type { AgentPresence, DocStatus, EngineProvider } from '@/lib/doc/provider'
import type { Api } from '@/rpc/bindings/client'
import { usePointerCoarse } from '@/lib/mobile'
import { deviceName } from '@/lib/platform'
import { subscribeDark } from '@/lib/theme'
import { cn } from 'cn'

/**
 * The schema of 59 §9.2 with the React frontmatter block and the UI's code block (languages table, diagram render);
 * identical ProseMirror shape to the helper's.
 */
const schema = createDocumentSchema(frontmatterBlock, codeBlock)
/** Code block colours: the app's shiki (`lib/shiki.ts`), loaded when the first code block needs it. */
const highlighter = SyntaxHighlightingExtension(CODE_BLOCK_OPTIONS)

/** Starter chart of the "Diagram" slash item: a second entry for `codeBlock`, `language: mermaid`. */
const DIAGRAM_STARTER = 'flowchart LR\n  A --> B'

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

/** Skips the Base UI handler merged onto the same element (`mergeProps`): only our handler runs. */
const preventBaseUI = (e: SyntheticEvent) => (e as SyntheticEvent & { preventBaseUIHandler?(): void }).preventBaseUIHandler?.()

/**
 * BlockNote's `DragHandleButton`, except the block menu opens on click. The shadcn flavour makes the handle a Base UI
 * `Menu.Trigger`, which opens on `mousedown`: a press-and-hold showed the menu and the drag started under it
 * (BLOCKNOTE-DRAG). Swallowing Base UI's pointerdown/mousedown leaves its `click` path, so a press that moves is a
 * native drag (no click follows a drag) and a press that does not is the menu.
 */
function DragHandle() {
  const Components = useComponentsContext()!
  const dict = useDictionary()
  const sideMenu = useExtension(SideMenuExtension)
  const block = useExtensionState(SideMenuExtension, { selector: (s) => s?.block })
  if (!block) return null
  return (
    <Components.Generic.Menu.Root onOpenChange={(open: boolean) => (open ? sideMenu.freezeMenu() : sideMenu.unfreezeMenu())} position="left">
      <Components.Generic.Menu.Trigger>
        <Components.SideMenu.Button
          label={dict.side_menu.drag_handle_label}
          draggable
          onDragStart={(e) => sideMenu.blockDragStart(e, block)}
          onDragEnd={sideMenu.blockDragEnd}
          className="bn-button"
          icon={<GripVertical className="size-5" data-test="dragHandle" />}
          {...({ onPointerDown: preventBaseUI, onMouseDown: preventBaseUI } as object)}
        />
      </Components.Generic.Menu.Trigger>
      <DragHandleMenu>
        <RemoveBlockItem>{dict.drag_handle.delete_menuitem}</RemoveBlockItem>
        <BlockColorsItem>{dict.drag_handle.colors_menuitem}</BlockColorsItem>
        <TableRowHeaderItem>{dict.drag_handle.header_row_menuitem}</TableRowHeaderItem>
        <TableColumnHeaderItem>{dict.drag_handle.header_column_menuitem}</TableColumnHeaderItem>
        <CellItems />
      </DragHandleMenu>
    </Components.Generic.Menu.Root>
  )
}

/**
 * 171 §4.2: the block menu's cell toggle on a code block — **Make it a cell** on a runnable language (shells, python)
 * without a header, **Make it plain code** on a cell. Each is one `updateBlock`: one transaction, one undo step.
 */
function CellItems() {
  const Components = useComponentsContext()!
  const editor = useBlockNoteEditor()
  const block = useExtensionState(SideMenuExtension, { selector: (s) => s?.block })
  if (!block || block.type !== 'codeBlock' || !editor.isEditable) return null
  const code = { id: block.id, props: { language: String(block.props.language ?? '') }, content: block.content }
  const text = Array.isArray(block.content) ? block.content.map((c) => ('text' in c ? c.text : '')).join('') : ''
  if (cellOf(code.props.language, text))
    return (
      <Components.Generic.Menu.Item className="bn-menu-item" onClick={() => makePlain(editor, code)}>
        Make it plain code
      </Components.Generic.Menu.Item>
    )
  if (!runnableLang(code.props.language)) return null
  return (
    <Components.Generic.Menu.Item className="bn-menu-item" onClick={() => makeCell(editor, code)}>
      Make it a cell
    </Components.Generic.Menu.Item>
  )
}

/** The default side menu, except on the frontmatter block: no drag handle, no block menu, no "+" (59 §4.5). The block menu adds the cell toggle (171 §4.2). */
function DocSideMenu(props: SideMenuProps) {
  const type = useExtensionState(SideMenuExtension, { selector: (s) => s?.block?.type })
  return type === 'frontmatter' ? null : (
    <SideMenu {...props}>
      <AddBlockButton />
      <DragHandle />
    </SideMenu>
  )
}

/**
 * The page frame (59 §24.y, §24.8): the scroll container is the canvas, `.bn-editor` the paper (styles.css `html .bn-editor`:
 * BlockNote's own sheet is unlayered, so utilities on `.bn-editor` would lose). The canvas is a column flex whose one
 * growing item is the paper, so an empty document is still page-high and the whole page is the editable element.
 * `dense` (the meeting pane, already in a bordered box) keeps the paper but tightens it.
 */
const CANVAS = 'min-h-0 flex-1 overflow-y-auto bg-background'

/**
 * BlockNote over a live `EngineProvider` (59 §3.3); the status line of §9.4 sits under it.
 * `onAsk(block)` is the "Ask theuth…" slash item (§9.3): the host opens a chat about this document and block.
 */
export function Editor({
  provider,
  status,
  className,
  onAsk,
  dense,
  guest,
}: {
  provider: EngineProvider
  status: DocStatus
  className?: string
  onAsk?: (block: string) => void
  dense?: boolean
  /** 171 §13: a share-link member's api — note cells show the replicated latest run and never run. */
  guest?: Api
}) {
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'))
  // 59 §10: fingers get the fixed toolbar instead of BlockNote's floating menus; labels on remote cursors need no hover.
  const coarse = usePointerCoarse()
  const [toolbar, setToolbar] = useState(false)
  // 184 §4.7: a re-sync keeps the caret and the keystrokes (the provider queues them); a first open waits.
  const editable = status.kind === 'live' || status.kind === 'offline' || (status.kind === 'connecting' && status.resync === true)
  const agentColor = provider.meta?.agent.color ?? '#7c3aed'
  const editor = useCreateBlockNote(
    withCollaboration({
      schema,
      // 71 §2.3: `code:` anchors keep their href in the editor too (render, paste) — the same gate as the helper's.
      links,
      collaboration: {
        provider,
        fragment: provider.doc.getXmlFragment(FRAGMENT),
        // 59 §8.5: no owner-name setting exists yet; the device label (`Mac`, `iPad`, `Chrome`) tells cursors
        // apart, the colour is picked by Yjs client id so two devices rarely share one.
        user: { name: deviceName(), color: CURSOR_COLORS[provider.doc.clientID % CURSOR_COLORS.length] as string },
        showCursorLabels: coarse ? 'always' : 'activity',
      },
      extensions: [revealExtension(provider, agentColor), highlighter],
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
  // 144 §6.3: `#<block>` in the location pulses that block; a `theuth://` link opens its target (no-op without a router).
  usePulseBlock()
  const openCite = useOpenCite()
  const extraItems = useMemo(
    () => [
      {
        key: 'diagram',
        title: 'Diagram',
        subtext: 'Mermaid chart, rendered in place',
        group: editor.dictionary.slash_menu.code_block.group,
        aliases: ['diagram', 'mermaid', 'chart', 'flowchart', 'graph'],
        icon: <Workflow className="size-4" />,
        onItemClick: () => insertOrUpdateBlockForSlashMenu(editor, { type: 'codeBlock', props: { language: 'mermaid' }, content: DIAGRAM_STARTER }),
      },
      {
        key: 'cell',
        title: 'Cell',
        subtext: 'Code you run with one press',
        group: editor.dictionary.slash_menu.code_block.group,
        aliases: ['cell', 'run', 'runnable', 'script', 'shell'],
        icon: <SquareTerminal className="size-4" />,
        onItemClick: () => insertOrUpdateBlockForSlashMenu(editor, CELL_STARTER),
      },
      ...(onAsk
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
        : []),
    ],
    [onAsk, editor],
  )
  const host = useMemo(() => ({ provider, guest: guest ?? null }), [provider, guest])
  return (
    <NoteHost value={host}>
    <div
      className={cn('flex min-h-0 flex-1 flex-col', className)}
      data-toolbar={toolbar || undefined}
      data-paper={dense ? 'dense' : 'page'}
      onClickCapture={(e) => {
        const href = (e.target as Element).closest('a[href^="theuth://"]')?.getAttribute('href')
        const cite = href ? parseTheuthHref(href) : null
        if (!cite) return
        e.preventDefault()
        e.stopPropagation()
        openCite(cite)
      }}
    >
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
                  [...getDefaultReactSlashMenuItems(editor).filter((item) => SLASH_KEYS[(item as { key?: string }).key ?? '']), ...extraItems],
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
        {editable && (
          <DictationButton
            onText={(text) => {
              if (!text) return
              editor.insertInlineContent(text)
              editor.focus()
            }}
          />
        )}
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
    </NoteHost>
  )
}

/** The agent's awareness state (59 §6.4, §9.4), re-read on every awareness change. */
function useAgentPresence(provider: EngineProvider): AgentPresence | null {
  const [agent, setAgent] = useState<AgentPresence | null>(() => provider.agentPresence())
  useEffect(() => {
    const read = () => setAgent(provider.agentPresence())
    read()
    provider.awareness.on('change', read)
    return () => provider.awareness.off('change', read)
  }, [provider])
  return agent
}
