import { markdown, markdownLanguage } from '@codemirror/lang-markdown'
import { languages } from '@codemirror/language-data'
import CodeMirror, { EditorView } from '@uiw/react-codemirror'
import { useSyncExternalStore } from 'react'
import { subscribeDark } from '@/lib/theme'
import { cn } from '@/lib/utils'

const extensions = [markdown({ base: markdownLanguage, codeLanguages: languages }), EditorView.lineWrapping]

/** Markdown editor for a charter body (the knowledge Editor's CodeMirror setup, without front matter or save keys). */
export function CharterEditor({ value, onChange, readOnly, className }: { value: string; onChange?: (v: string) => void; readOnly?: boolean; className?: string }) {
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'))
  return (
    <CodeMirror
      value={value}
      readOnly={readOnly}
      editable={!readOnly}
      theme={dark ? 'dark' : 'light'}
      extensions={extensions}
      basicSetup={{ lineNumbers: false, foldGutter: false, highlightActiveLine: false }}
      className={cn(
        'min-h-0 overflow-auto rounded-lg border border-border-subtle text-[14px] [&_.cm-editor]:h-full [&_.cm-editor]:bg-transparent [&_.cm-focused]:outline-none [&_.cm-gutters]:bg-transparent',
        className,
      )}
      height="100%"
      onChange={onChange}
    />
  )
}
