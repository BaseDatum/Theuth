import { createCodeBlockSpec, plainContentToString } from '@blocknote/core'
import { type ReactCustomBlockRenderProps, createReactBlockSpec, useEditorState } from '@blocknote/react'
import { PencilLine } from 'lucide-react'
import { type ReactNode, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { CellDefaults } from '@/components/cells/CellDefaults'
import { GuestCell, useLatest } from '@/components/cells/GuestCell'
import { NoteCell } from '@/components/cells/NoteCell'
import { NoteHost } from '@/components/cells/note-context'
import { firstDefaults, noteRef, routeOf } from '@/components/cells/note-edit'
import { loadMermaid, MermaidFence } from '@/components/chat/Mermaid'
import { JsonRenderFence } from '@/components/chat/surface/JsonRenderFence'
import { parseSpec } from '@/components/chat/surface/parse'
import { CODE_BLOCK_OPTIONS, type Diagram, diagramOf } from '@/components/editor/codeLanguages'
import { Button } from '@/components/ui/button'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { Api } from '@/rpc/api'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import { cn } from 'cn'

/**
 * BlockNote's own code block built with our language table (59 §9.2 as-built). Its config, parser, meta (the
 * `highlight` hook the shiki extension reads) and keyboard extension (Tab, Enter, ```` ```lang ```` rule) are reused
 * below verbatim; only `render` is ours, so the ProseMirror node — and the Yjs fragment — are the default's.
 */
const base = createCodeBlockSpec(CODE_BLOCK_OPTIONS)
const { supportedLanguages } = CODE_BLOCK_OPTIONS

type Props = ReactCustomBlockRenderProps<typeof base.config>

/** Visually hidden but in the DOM, so the caret can still land in the source (arrow keys, `setTextCursorPosition`). */
const PARKED = 'absolute! h-px! w-px! overflow-hidden! p-0! opacity-0'

/**
 * Whether `source` renders as `kind`: `false` also for empty source, `null` while mermaid is still parsing.
 * ponytail: mermaid is parsed on every keystroke while the block is edited; debounce if a large chart feels slow.
 */
function useParses(kind: Diagram | null, source: string): boolean | null {
  const [ok, setOk] = useState<boolean | null>(null)
  useEffect(() => {
    if (kind === null) return
    if (source.trim() === '') {
      setOk(false)
      return
    }
    if (kind === 'json-render') {
      setOk(!('error' in parseSpec(source)))
      return
    }
    let live = true
    setOk(null)
    // Through the lazy loader (65 §4): a static `mermaid` import here would put the core back in the entry chunk.
    loadMermaid()
      .then((m) => m.parse(source, { suppressErrors: true }))
      .then(
        (r) => live && setOk(r !== false),
        () => live && setOk(false),
      )
    return () => {
      live = false
    }
  }, [kind, source])
  return ok
}

const INTERACTIVE = 'button, a, input, select, textarea, [role=button], [role=dialog]'

/** `e` happened in this element's own DOM (React also bubbles portal events — the fullscreen dialog's — up the tree). */
const own = (e: { currentTarget: HTMLElement; target: EventTarget | null }) =>
  e.target instanceof Element && e.currentTarget.contains(e.target) && !e.target.closest(INTERACTIVE)

/**
 * The rendered surface: static (the fence's pan/zoom never sees a pointer or wheel event, fingers scroll the note),
 * one click on it or the button opens the source. Fullscreen and export stay on the fence's own toolbar.
 */
function DiagramView({ kind, source, parses, editable, onEdit }: { kind: Diagram; source: string; parses: boolean | null; editable: boolean; onEdit: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = ref.current!
    const stop = (e: WheelEvent) => e.stopPropagation()
    el.addEventListener('wheel', stop, { capture: true })
    return () => el.removeEventListener('wheel', stop, { capture: true })
  }, [])
  let surface: ReactNode
  if (parses === null) surface = <div className="flex min-h-28 items-center justify-center text-xs text-fg-tertiary">Rendering diagram…</div>
  else if (kind === 'mermaid') surface = <MermaidFence code={source} language={kind} isIncomplete={false} />
  else surface = <JsonRenderFence code={source} language={kind} isIncomplete={false} />
  return (
    // Keyboard path: the arrow keys land the caret in the parked source, which shows it.
    <div
      ref={ref}
      data-diagram={kind}
      contentEditable={false}
      className={cn('w-full [&_[data-streamdown]]:my-0 [&_*]:touch-auto!', editable && 'cursor-pointer')}
      onClick={(e) => editable && own(e) && onEdit()}
      onPointerDownCapture={(e) => own(e) && e.stopPropagation()}
    >
      {editable && (
        <div className="flex justify-end">
          <Button variant="ghost" size="sm" className="text-fg-tertiary pointer-coarse:min-h-11 pointer-coarse:min-w-11" onClick={onEdit}>
            <PencilLine />
            Edit source
          </Button>
        </div>
      )}
      {/* The fence's toolbar uses Tooltip; the share-link guest page (`GuestApp`) mounts no sidebar, hence no provider. */}
      <TooltipProvider>{surface}</TooltipProvider>
    </div>
  )
}

/** A share-link member's note cell (§13): the replicated latest row through the member's own api, no Run. */
function GuestBlock({ api, doc, spec, source, block, editor }: { api: Api; doc: string; spec: CellSpec; source: string; block: string; editor: ReactNode }) {
  const ref = noteRef(doc, spec, block)
  return <GuestCell cell={spec} code={source} latest={useLatest(api, doc, ref.key.value)} cellRef={ref} editor={editor} />
}

/**
 * Source is shown when the block is code, has the caret or a selection, or does not parse; otherwise the diagram.
 * Leaving the block (the selection moves out) is what returns to the diagram, so no toggle state lives outside
 * the editor. `mermaid`/`json-render` aliases count (`diagramOf`).
 */
function CodeBlock({ block, editor, contentRef }: Props) {
  const kind = diagramOf(block.props.language)
  const source = plainContentToString(block.content)
  const { selected, editable } = useEditorState({
    editor,
    selector: ({ editor }) => {
      let selected = false
      try {
        selected = editor.getTextCursorPosition().block.id === block.id
      } catch {
        // NodeSelection: no text cursor.
      }
      selected ||= editor.getSelection()?.blocks.some((b) => b.id === block.id) ?? false
      return { selected, editable: editor.isEditable }
    },
  })
  const parses = useParses(kind, source)
  // 171 §4.2 (D14): cells and the `cells` defaults line come before the diagram branch (`routeOf`); the node and its
  // Yjs text are the default's.
  const host = useContext(NoteHost)
  const route = useMemo(() => routeOf(block.props.language, source, host, selected), [block.props.language, source, host, selected])
  const open = () => {
    editor.setTextCursorPosition(block.id, 'end')
    editor.focus()
  }
  if (host && (route.kind === 'cell' || route.kind === 'guest')) {
    const code = (
      <pre data-cell-code="" spellCheck={false}>
        <code ref={contentRef} />
      </pre>
    )
    if (route.kind === 'guest' && host.guest) return <GuestBlock api={host.guest} doc={host.provider.id} spec={route.spec} source={source} block={block.id} editor={code} />
    const key = noteRef(host.provider.id, route.spec, block.id).key
    return <NoteCell doc={host.provider.id} key_={key} cell={route.spec} code={source} editor={code} editable={editable} block={block.id} blocknote={editor} />
  }
  // The `cells` block shows its one line until the caret enters it; the source stays mounted (parked), like a diagram's.
  const defaults = route.kind === 'defaults'
  const diagram = kind !== null && !selected && parses !== false
  return (
    <>
      {defaults && <CellDefaults text={source} second={!firstDefaults(editor.document, block.id)} onOpen={open} />}
      {diagram && <DiagramView kind={kind} source={source} parses={parses} editable={editable} onEdit={open} />}
      <div contentEditable={false} className={cn((diagram || defaults) && 'hidden')}>
        <select
          value={block.props.language}
          disabled={!editable}
          onChange={(e) => editor.updateBlock(block.id, { props: { language: e.target.value } })}
        >
          {Object.entries(supportedLanguages).map(([id, { name }]) => (
            <option key={id} value={id}>
              {name}
            </option>
          ))}
          {!(block.props.language in supportedLanguages) && <option value={block.props.language}>{block.props.language}</option>}
        </select>
      </div>
      <pre className={cn((diagram || defaults) && PARKED)} aria-hidden={diagram || defaults || undefined}>
        <code ref={contentRef} />
      </pre>
      {kind !== null && !diagram && parses === false && (
        <p contentEditable={false} className="m-0 px-3 pb-2 text-xs text-fg-tertiary">
          diagram will render when the source parses
        </p>
      )}
    </>
  )
}

/** The `codeBlock` spec the editor's schema uses: the default's config, parser, meta and shortcuts; this render. */
export const codeBlock = createReactBlockSpec(
  base.config,
  {
    meta: base.implementation.meta,
    parse: base.implementation.parse,
    parseContent: base.implementation.parseContent,
    render: CodeBlock,
    // Copy / external HTML: the default's `<pre><code class="language-…">`, never the diagram.
    toExternalHTML: ({ block, contentRef }) => (
      <pre>
        <code ref={contentRef} className={`language-${block.props.language}`} data-language={block.props.language} />
      </pre>
    ),
  },
  base.extensions,
)()
