import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { tags } from '@lezer/highlight'
import { EditorView } from '@uiw/react-codemirror'

const editorSurface = EditorView.theme({
  '&': {
    backgroundColor: 'transparent',
    color: 'var(--fg-primary)',
  },
  '&.cm-focused': {
    outline: 'none',
  },
  '.cm-scroller': {
    fontFamily: 'inherit',
    lineHeight: '1.6',
  },
  '.cm-content': {
    caretColor: 'var(--fg-primary)',
    padding: '10px 12px',
  },
  '.cm-cursor, .cm-dropCursor': {
    borderLeftColor: 'var(--fg-accent)',
  },
  '.cm-gutters': {
    backgroundColor: 'transparent',
    color: 'var(--fg-tertiary)',
    border: 'none',
  },
  '.cm-activeLine, .cm-activeLineGutter': {
    backgroundColor: 'transparent',
  },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': {
    backgroundColor: 'color-mix(in srgb, var(--fg-accent) 24%, transparent)',
  },
  '.cm-matchingBracket': {
    backgroundColor: 'color-mix(in srgb, var(--fg-accent) 20%, transparent)',
    color: 'var(--fg-primary)',
  },
  '.cm-searchMatch': {
    backgroundColor: 'color-mix(in srgb, var(--fg-accent) 26%, transparent)',
  },
  '.cm-panels': {
    backgroundColor: 'var(--bg-layer-1)',
    color: 'var(--fg-primary)',
    borderColor: 'var(--border-subtle)',
  },
  '.cm-placeholder': {
    color: 'var(--fg-tertiary)',
  },
})

const highlight = HighlightStyle.define([
  { tag: tags.heading, color: 'var(--fg-primary)', fontWeight: '600' },
  { tag: tags.strong, color: 'var(--fg-primary)', fontWeight: '600' },
  { tag: tags.emphasis, color: 'var(--fg-secondary)', fontStyle: 'italic' },
  { tag: tags.strikethrough, color: 'var(--fg-tertiary)', textDecoration: 'line-through' },
  { tag: [tags.link, tags.url], color: 'var(--fg-accent)' },
  { tag: tags.monospace, color: 'var(--chart-1)' },
  { tag: tags.quote, color: 'var(--fg-secondary)', fontStyle: 'italic' },
  { tag: [tags.meta, tags.comment, tags.processingInstruction, tags.punctuation], color: 'var(--fg-tertiary)' },
  { tag: [tags.keyword, tags.operatorKeyword], color: 'var(--fg-accent)' },
  { tag: tags.string, color: 'var(--chart-4)' },
  { tag: [tags.number, tags.bool, tags.atom], color: 'var(--chart-2)' },
  { tag: [tags.propertyName, tags.attributeName], color: 'var(--chart-1)' },
])

/** CodeMirror theme + syntax highlighting driven by theuth's CSS tokens (matches light & dark themes). */
export const theuthEditorTheme = [editorSurface, syntaxHighlighting(highlight)]
