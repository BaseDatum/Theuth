// Owner: U1
import { EditorView, highlightSpecialChars } from '@uiw/react-codemirror'

/**
 * 171 §11.8: CodeMirror theme reading theuth's CSS variables (`--fg-tertiary`, `--fg-accent`), built once at module
 * scope (171-recon-theirs §16 trap 7: a new extension array per render rebuilds the editor). Importers spread it into
 * their extension list. Syntax colours are shiki's (`shiki-marks.ts`). §3.7: invisible characters are drawn as placeholders.
 */
export const cellTheme = [
  EditorView.theme({
    '&': { backgroundColor: 'transparent', color: 'var(--foreground)', fontSize: '13px' },
    '&.cm-focused': { outline: 'none' },
    '.cm-scroller': { fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace', lineHeight: '1.6' },
    '.cm-content': { caretColor: 'var(--foreground)', padding: '6px 0' },
    '.cm-gutters': { backgroundColor: 'transparent', color: 'var(--fg-tertiary)', border: 'none' },
    '.cm-lineNumbers .cm-gutterElement': { padding: '0 8px 0 10px', minWidth: '28px' },
    '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'transparent' },
    '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': {
      backgroundColor: 'color-mix(in srgb, var(--fg-accent) 22%, transparent)',
    },
    '.cm-specialChar': { color: 'var(--status-danger-fg)', backgroundColor: 'var(--status-danger-bg)' },
    '.cm-run-gutter .cm-gutterElement': { width: '18px', color: 'var(--fg-accent)', cursor: 'pointer', textAlign: 'center' },
    // 219: the per-line copy marker beside the ►.
    '.cm-copy-gutter .cm-gutterElement': { width: '18px', color: 'var(--fg-tertiary)', cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center' },
  }),
  highlightSpecialChars({ addSpecialChars: /[\u200b-\u200f\u202a-\u202e\u2066-\u2069\ufeff]/ }),
]
