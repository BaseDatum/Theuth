import type { BlockNoteEditor } from '@blocknote/core'
import { useEditorState } from '@blocknote/react'
import type { DocumentSchema, PartialDocBlock } from '@theuth/documents/schema'
import { Bold, Code, IndentDecrease, IndentIncrease, Italic, KeyboardOff, Link2, Redo2, Strikethrough, Undo2 } from 'lucide-react'
import { type CSSProperties, type ComponentType, type ReactNode, type SyntheticEvent, useEffect, useRef, useState } from 'react'
import { createPortal, flushSync } from 'react-dom'
import { BLOCK_TYPES, type BlockTypeKey, GRACE_MS, type Visibility, blockTypeKey, nextVisibility, toolbarVisible } from '@/components/editor/toolbar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useKeyboardInset } from '@/lib/mobile'
import { cn } from 'cn'

export type DocEditor = BlockNoteEditor<DocumentSchema['blockSchema'], DocumentSchema['inlineContentSchema'], DocumentSchema['styleSchema']>

/**
 * Keep the editor's focus (and the iOS keyboard) when the bar is tapped, 59 §10. `pointerdown` alone is not enough
 * in WKWebView: the blur happens with the tap's synthesized mouse events, so `touchend` (which precedes them)
 * and `mousedown` are cancelled too. Not `touchstart`: that would also cancel the groups' horizontal pan. Because a
 * cancelled `touchend` emits no `click`, the buttons act on `pointerup` (dispatched before `touchend`; a pan
 * ends in `pointercancel`, so a scroll never fires an action).
 */
const keepFocus = (e: SyntheticEvent) => e.preventDefault()

/** What the buttons reflect, re-read on every editor transaction; `null` block = the cursor is not in a text block. */
function toolbarState(editor: DocEditor) {
  let block: BlockTypeKey | null = null
  let text = false
  try {
    block = blockTypeKey(editor.getTextCursorPosition().block)
    text = true
  } catch {
    // NodeSelection (frontmatter) or an empty document: nothing to reflect.
  }
  const styles = text ? editor.getActiveStyles() : {}
  return {
    block,
    text,
    bold: styles.bold === true,
    italic: styles.italic === true,
    strike: styles.strike === true,
    code: styles.code === true,
    link: text ? (editor.getSelectedLinkUrl() ?? null) : null,
    canNest: text && editor.canNestBlock(),
    canUnnest: text && editor.canUnnestBlock(),
  }
}

/**
 * The fixed bar of 59 §10 under a coarse pointer, in a portal at `bottom: var(--kb-inset)`; shown while the
 * editor is focused (150 ms grace on blur), gone when read-only. `onVisible` lets the editor pad its scroll box.
 */
export function MobileToolbar({ editor, readonly, onVisible }: { editor: DocEditor; readonly: boolean; onVisible: (v: boolean) => void }) {
  const inset = useKeyboardInset()
  const [focus, setFocus] = useState<Visibility>(() => (editor.isFocused() ? 'shown' : 'hidden'))
  // The block-type picker is a row above the bar, inside the focus guard: opening it never blurs the editor.
  const [picker, setPicker] = useState(false)
  // The link sheet needs an input, so it takes the focus; `link` stays set through the close animation (the bar
  // must not blink) and is cleared in `onCloseAutoFocus`, where Radix would otherwise refocus the trigger.
  const [link, setLink] = useState(false)
  const [linkOpen, setLinkOpen] = useState(false)
  const timer = useRef(0)
  useEffect(() => {
    // `focusin`/`focusout` on `document`: the ProseMirror root may not exist yet, and the frontmatter's inputs
    // (inside the root) must not count — a text cursor is what the buttons act on.
    const on = (e: FocusEvent) => {
      if (e.target !== editor.domElement) return
      clearTimeout(timer.current)
      if (e.type === 'focusin') setFocus((v) => nextVisibility(v, 'focus'))
      else {
        setFocus((v) => nextVisibility(v, 'blur'))
        timer.current = window.setTimeout(() => setFocus((v) => nextVisibility(v, 'elapsed')), GRACE_MS)
      }
    }
    document.addEventListener('focusin', on)
    document.addEventListener('focusout', on)
    return () => {
      document.removeEventListener('focusin', on)
      document.removeEventListener('focusout', on)
      clearTimeout(timer.current)
    }
  }, [editor])
  const visible = toolbarVisible(focus, readonly, link)
  useEffect(() => onVisible(visible), [visible, onVisible])
  // The bar went away (keyboard down, read-only): the picker must not be open when it comes back.
  useEffect(() => {
    if (!visible) setPicker(false)
  }, [visible])
  // Selection change: keep the cursor's block above the bar (WebKit does not do this for a fixed element, §10).
  useEffect(() => {
    if (!visible) return
    return editor.onSelectionChange(() => {
      try {
        const id = editor.getTextCursorPosition().block.id
        editor.domElement?.querySelector(`.bn-block[data-id="${id}"] > .bn-block-content`)?.scrollIntoView({ block: 'nearest' })
      } catch {
        // no text cursor
      }
    })
  }, [editor, visible])
  const s = useEditorState({ editor, selector: ({ editor }) => toolbarState(editor as DocEditor) })
  if (!visible) return null

  /** Run an action, then refocus synchronously — still inside the tap, which is when iOS honours a programmatic focus. */
  const act = (f: () => void) => () => {
    f()
    editor.focus()
  }
  // Apply/Remove/dismiss: close first, synchronously, so the dialog's focus trap is gone (`trapped` follows `open`),
  // then refocus while still inside the tap. `onCloseAutoFocus` (after the exit animation) is the belt-and-braces:
  // cancelled so Radix does not hand focus back to the trigger, then the editor once more.
  const closeLink = () => {
    flushSync(() => setLinkOpen(false))
    editor.focus()
  }
  const linkClosed = (e: Event) => {
    e.preventDefault()
    setLink(false)
    editor.focus()
  }
  const setType = (t: (typeof BLOCK_TYPES)[number]) => {
    const selection = editor.getSelection()
    const blocks = selection?.blocks ?? [editor.getTextCursorPosition().block]
    let last: string | undefined
    for (const b of blocks) {
      if (b.type === 'frontmatter') continue
      // The chip table is `as const`; TS cannot correlate `type` with `props` across the union, the rows are valid by construction.
      const update = ('props' in t ? { type: t.type, props: t.props } : { type: t.type }) as PartialDocBlock
      editor.updateBlock(b, update)
      last = b.id
    }
    // A collapsed cursor continues at the end of the new block (ids survive `updateBlock`); a selection is kept as is.
    if (last && !selection) editor.setTextCursorPosition(last, 'end')
    setPicker(false)
  }
  const current = BLOCK_TYPES.find((t) => t.key === s.block)

  return createPortal(
    <>
      <div
        data-mobile-toolbar
        className="fixed inset-x-0 z-40 flex flex-col border-t border-border-subtle bg-background"
        style={{ bottom: 'var(--kb-inset)', '--kb-inset': `calc(${inset}px + var(--safe-bottom))` } as CSSProperties}
        onPointerDown={keepFocus}
        onMouseDown={keepFocus}
        onTouchEnd={keepFocus}
      >
        {picker && (
          <div role="toolbar" aria-label="Block type" className="flex items-center gap-1 overflow-x-auto border-b border-border-subtle px-1 py-1 [scrollbar-width:none]">
            {BLOCK_TYPES.map((t) => (
              <Tool
                key={t.key}
                label={t.label}
                pressed={t.key === s.block}
                className="h-11 w-auto px-3 text-base font-normal"
                data-block-type={t.key}
                onTap={act(() => setType(t))}
              >
                {t.label}
              </Tool>
            ))}
          </div>
        )}
        <div role="toolbar" aria-label="Formatting" className="flex items-center py-1">
          {/* Twelve 44 px targets do not fit a 390 px phone: the groups scroll, keyboard-down stays pinned. */}
          <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto px-1 [scrollbar-width:none]">
            <Tool label={current ? current.label : 'Block type'} pressed={picker} disabled={!s.text} onTap={act(() => setPicker((p) => !p))}>
              <span className="text-[15px] font-semibold">Aa</span>
            </Tool>
            <Sep />
            <Tool label="Bold" icon={Bold} pressed={s.bold} disabled={!s.text} onTap={act(() => editor.toggleStyles({ bold: true }))} />
            <Tool label="Italic" icon={Italic} pressed={s.italic} disabled={!s.text} onTap={act(() => editor.toggleStyles({ italic: true }))} />
            <Tool label="Strikethrough" icon={Strikethrough} pressed={s.strike} disabled={!s.text} onTap={act(() => editor.toggleStyles({ strike: true }))} />
            <Tool label="Code" icon={Code} pressed={s.code} disabled={!s.text} onTap={act(() => editor.toggleStyles({ code: true }))} />
            <Sep />
            <Tool label="Outdent" icon={IndentDecrease} disabled={!s.canUnnest} onTap={act(() => editor.unnestBlock())} />
            <Tool label="Indent" icon={IndentIncrease} disabled={!s.canNest} onTap={act(() => editor.nestBlock())} />
            <Sep />
            <Tool
              label="Link"
              icon={Link2}
              pressed={s.link !== null}
              disabled={!s.text}
              onTap={() => {
                setPicker(false)
                setLink(true)
                setLinkOpen(true)
              }}
            />
            <Sep />
            <Tool label="Undo" icon={Undo2} onTap={act(() => editor.undo())} />
            <Tool label="Redo" icon={Redo2} onTap={act(() => editor.redo())} />
          </div>
          <Sep />
          {/* The one button that is meant to blur. */}
          <Tool
            label="Hide keyboard"
            icon={KeyboardOff}
            className="mr-1"
            onTap={() => {
              editor.domElement?.blur()
              if (document.activeElement instanceof HTMLElement) document.activeElement.blur()
            }}
          />
        </div>
      </div>
      <Sheet open={linkOpen} onOpenChange={(o) => !o && closeLink()}>
        <SheetContent side="bottom" showCloseButton={false} className="p-3" onCloseAutoFocus={linkClosed}>
          <SheetTitle className="text-xs font-medium text-fg-tertiary">Link</SheetTitle>
          <LinkForm
            initial={s.link ?? ''}
            onApply={(url) => {
              if (s.link) {
                const at = editor.prosemirrorView.state.selection.anchor
                editor.editLink(url, editor.getLinkMarkAtPos(at)?.text ?? url, at)
              } else editor.createLink(url, editor.getSelectedText() ? undefined : url)
              closeLink()
            }}
            onRemove={
              s.link
                ? () => {
                    editor.deleteLink()
                    closeLink()
                  }
                : undefined
            }
          />
        </SheetContent>
      </Sheet>
    </>,
    document.body,
  )
}

function LinkForm({ initial, onApply, onRemove }: { initial: string; onApply: (url: string) => void; onRemove?: () => void }) {
  const [url, setUrl] = useState(initial)
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault()
        if (url.trim()) onApply(url.trim())
      }}
    >
      <Input
        type="url"
        inputMode="url"
        autoFocus
        placeholder="https://…"
        aria-label="URL"
        className="h-11 text-base md:text-base"
        value={url}
        onChange={(e) => setUrl(e.target.value)}
      />
      <div className="flex gap-2">
        <Button type="submit" className="h-11 flex-1" disabled={!url.trim()}>
          Apply
        </Button>
        {onRemove && (
          <Button type="button" variant="outline" className="h-11 flex-1" onClick={onRemove}>
            Remove link
          </Button>
        )}
      </div>
    </form>
  )
}

/** One 44 px target; `pressed` is the active state (bold on, picker open). Never a focus target (`tabIndex -1`); see `keepFocus`. */
function Tool({
  label,
  icon: Icon,
  pressed,
  disabled,
  className,
  onTap,
  children,
  ...rest
}: {
  label: string
  icon?: ComponentType<{ className?: string }>
  pressed?: boolean
  disabled?: boolean
  className?: string
  onTap: () => void
  children?: ReactNode
  'data-block-type'?: string
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      tabIndex={-1}
      className={cn('size-11 shrink-0 rounded-lg', pressed && 'bg-accent text-accent-foreground', className)}
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      disabled={disabled}
      onPointerUp={onTap}
      {...rest}
    >
      {Icon ? <Icon className="size-5" /> : children}
    </Button>
  )
}

const Sep = () => <span className="mx-0.5 h-6 w-px shrink-0 bg-border-subtle" aria-hidden />
