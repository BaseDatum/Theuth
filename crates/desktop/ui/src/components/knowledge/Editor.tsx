import { markdown, markdownLanguage } from '@codemirror/lang-markdown'
import { yamlFrontmatter } from '@codemirror/lang-yaml'
import { languages } from '@codemirror/language-data'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import CodeMirror, { EditorView } from '@uiw/react-codemirror'
import { useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import { subscribeDark } from '@/lib/theme'
import type { Meta } from '@/rpc/bindings/Meta'

const extensions = [yamlFrontmatter({ content: markdown({ base: markdownLanguage, codeLanguages: languages }) }), EditorView.lineWrapping]

type Props = {
  id: string
  initial: string
  onSaved: (meta: Meta) => void
  onCancel: () => void
}

/** CodeMirror over the whole file; ⌘S writes, Esc leaves (asking first when dirty). */
export function Editor({ id, initial, onSaved, onCancel }: Props) {
  const api = useApi()
  const qc = useQueryClient()
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'))
  const [text, setText] = useState(initial)
  const [error, setError] = useState<string | null>(null)
  const [confirmDiscard, setConfirmDiscard] = useState(false)
  const dirty = text !== initial

  const save = useMutation({
    mutationFn: (t: string) => api.knowledge_write(id, t),
    onSuccess: (meta) => {
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeDoc(id) })
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeList })
      toast('Saved')
      onSaved(meta)
    },
    onError: (e) => setError(e.message),
  })

  const leave = () => (dirty ? setConfirmDiscard(true) : onCancel())

  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      onKeyDown={(e) => {
        // CodeMirror marks keys it consumed (Esc closing its search panel) as default-prevented; the dialog handles its own.
        if (e.defaultPrevented || confirmDiscard) return
        if (e.key === 's' && (e.metaKey || e.ctrlKey)) {
          e.preventDefault()
          save.mutate(text)
        } else if (e.key === 'Escape') {
          e.preventDefault()
          leave()
        }
      }}
    >
      <CodeMirror
        value={text}
        autoFocus
        theme={dark ? 'dark' : 'light'}
        extensions={extensions}
        basicSetup={{ lineNumbers: false, foldGutter: false, highlightActiveLine: false }}
        className="min-h-0 flex-1 overflow-auto text-[15px] [&_.cm-editor]:h-full [&_.cm-editor]:bg-transparent [&_.cm-focused]:outline-none [&_.cm-gutters]:bg-transparent"
        height="100%"
        onChange={(v) => {
          setText(v)
          setError(null)
        }}
      />
      <footer className="flex shrink-0 items-center gap-2 border-t border-border-subtle px-4 py-2">
        {error && <p className="min-w-0 flex-1 truncate text-sm text-danger">{error}</p>}
        <span className="flex-1" />
        <Button variant="ghost" size="sm" onClick={leave}>
          Cancel
        </Button>
        <Button size="sm" disabled={save.isPending} onClick={() => save.mutate(text)}>
          Save
          <kbd className="ml-1 text-xs opacity-60">⌘S</kbd>
        </Button>
      </footer>
      <AlertDialog open={confirmDiscard} onOpenChange={setConfirmDiscard}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Discard changes?</AlertDialogTitle>
            <AlertDialogDescription>Unsaved edits to {id} will be lost.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <AlertDialogAction onClick={onCancel}>Discard</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
