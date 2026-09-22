import type { BlockNoteEditor } from '@blocknote/core'
import { useEditorState } from '@blocknote/react'
import type { DocumentSchema } from '@theuth/documents/schema'
import { Bold, Code, IndentDecrease, IndentIncrease, Italic, KeyboardOff, Link2, Redo2, Strikethrough, Undo2 } from 'lucide-react'
import { type CSSProperties, type ComponentType, type PointerEvent, type ReactNode, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { BLOCK_TYPES, type BlockTypeKey, GRACE_MS, type Visibility, blockTypeKey, nextVisibility, toolbarVisible } from '@/components/editor/toolbar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useKeyboardInset } from '@/lib/mobile'
import { cn } from 'cn'

export type DocEditor = BlockNoteEditor<DocumentSchema['blockSchema'], DocumentSchema['inlineContentSchema'], DocumentSchema['styleSchema']>

/** Keep the editor's focus (and the iOS keyboard) when a toolbar button is tapped. */
const keepFocus = (e: PointerEvent) => e.preventDefault()

type SheetKind = 'type' | 'link' | null

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
  // `sheet` is which sheet is mounted — it stays through the close animation, so the bar does not blink; `open`
  // drives Radix. Focus goes back to the editor only in `onCloseAutoFocus`: earlier, the dialog's trap pulls it back.
  const [sheet, setSheet] = useState<SheetKind>(null)
  const [open, setOpen] = useState(false)
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
  const visible = toolbarVisible(focus, readonly, sheet !== null)
  useEffect(() => onVisible(visible), [visible, onVisible])
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

  const show = (k: SheetKind) => {
    setSheet(k)
    setOpen(true)
  }
  const close = () => setOpen(false)
  const closed = (e: Event) => {
    e.preventDefault()
    setSheet(null)
    editor.focus()
  }
  const setType = (t: (typeof BLOCK_TYPES)[number]) => {
    const blocks = editor.getSelection()?.blocks ?? [editor.getTextCursorPosition().block]
    for (const b of blocks) {
      if (b.type === 'frontmatter') continue
      editor.updateBlock(b, 'props' in t ? { type: t.type, props: t.props } : { type: t.type })
    }
    close()
  }
  const current = BLOCK_TYPES.find((t) => t.key === s.block)

  return createPortal(
    <>
      <div
        role="toolbar"
        aria-label="Formatting"
        data-mobile-toolbar
        className="fixed inset-x-0 z-40 flex items-center border-t border-border-subtle bg-background py-1"
        style={{ bottom: 'var(--kb-inset)', '--kb-inset': `calc(${inset}px + var(--safe-bottom))` } as CSSProperties}
        onPointerDown={keepFocus}
      >
        {/* Twelve 44 px targets do not fit a 390 px phone: the groups scroll, keyboard-down stays pinned. */}
        <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto px-1 [scrollbar-width:none]">
          <Tool label={current ? current.label : 'Block type'} pressed={open && sheet === 'type'} disabled={!s.text} onClick={() => show('type')}>
            <span className="text-[15px] font-semibold">Aa</span>
          </Tool>
          <Sep />
          <Tool label="Bold" icon={Bold} pressed={s.bold} disabled={!s.text} onClick={() => editor.toggleStyles({ bold: true })} />
          <Tool label="Italic" icon={Italic} pressed={s.italic} disabled={!s.text} onClick={() => editor.toggleStyles({ italic: true })} />
          <Tool label="Strikethrough" icon={Strikethrough} pressed={s.strike} disabled={!s.text} onClick={() => editor.toggleStyles({ strike: true })} />
          <Tool label="Code" icon={Code} pressed={s.code} disabled={!s.text} onClick={() => editor.toggleStyles({ code: true })} />
          <Sep />
          <Tool label="Outdent" icon={IndentDecrease} disabled={!s.canUnnest} onClick={() => editor.unnestBlock()} />
          <Tool label="Indent" icon={IndentIncrease} disabled={!s.canNest} onClick={() => editor.nestBlock()} />
          <Sep />
          <Tool label="Link" icon={Link2} pressed={s.link !== null} disabled={!s.text} onClick={() => show('link')} />
          <Sep />
          <Tool label="Undo" icon={Undo2} onClick={() => editor.undo()} />
          <Tool label="Redo" icon={Redo2} onClick={() => editor.redo()} />
        </div>
        <Sep />
        <Tool
          label="Hide keyboard"
          icon={KeyboardOff}
          className="mr-1"
          onClick={() => {
            editor.domElement?.blur()
            if (document.activeElement instanceof HTMLElement) document.activeElement.blur()
          }}
        />
      </div>
      <Sheet open={open && sheet === 'type'} onOpenChange={(o) => !o && close()}>
        <SheetContent side="bottom" showCloseButton={false} className="gap-1 p-2" onCloseAutoFocus={closed}>
          <SheetTitle className="px-2 pt-1 text-xs font-medium text-fg-tertiary">Block type</SheetTitle>
          {BLOCK_TYPES.map((t) => (
            <Button
              key={t.key}
              variant="ghost"
              className="h-11 justify-start text-base"
              aria-pressed={t.key === s.block}
              data-block-type={t.key}
              onClick={() => setType(t)}
            >
              <span className={cn('w-4 text-center', t.key !== s.block && 'invisible')}>•</span>
              {t.label}
            </Button>
          ))}
        </SheetContent>
      </Sheet>
      <Sheet open={open && sheet === 'link'} onOpenChange={(o) => !o && close()}>
        <SheetContent side="bottom" showCloseButton={false} className="p-3" onCloseAutoFocus={closed}>
          <SheetTitle className="text-xs font-medium text-fg-tertiary">Link</SheetTitle>
          <LinkForm
            initial={s.link ?? ''}
            onApply={(url) => {
              if (s.link) {
                const at = editor.prosemirrorView.state.selection.anchor
                editor.editLink(url, editor.getLinkMarkAtPos(at)?.text ?? url, at)
              } else editor.createLink(url, editor.getSelectedText() ? undefined : url)
              close()
            }}
            onRemove={
              s.link
                ? () => {
                    editor.deleteLink()
                    close()
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

/** One 44 px target; `pressed` is the active state (bold on, sheet open). */
function Tool({
  label,
  icon: Icon,
  pressed,
  disabled,
  className,
  onClick,
  children,
}: {
  label: string
  icon?: ComponentType<{ className?: string }>
  pressed?: boolean
  disabled?: boolean
  className?: string
  onClick: () => void
  children?: ReactNode
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      className={cn('size-11 shrink-0 rounded-lg', pressed && 'bg-accent text-accent-foreground', className)}
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      disabled={disabled}
      onClick={onClick}
    >
      {Icon ? <Icon className="size-5" /> : children}
    </Button>
  )
}

const Sep = () => <span className="mx-0.5 h-6 w-px shrink-0 bg-border-subtle" aria-hidden />
