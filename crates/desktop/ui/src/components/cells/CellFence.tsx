// Owner: U1
import { CodeBlock, type CustomRendererProps } from 'streamdown'
import { cellOf } from './grammar'

/** 171 §4.1: the fence languages `CellFence` registers for. */
export const CELL_LANGUAGES = ['bash', 'zsh', 'sh', 'pwsh', 'powershell', 'python', 'py']

/**
 * 171 §4.1: a streamdown renderer for shell and python fences. No `#|` header → streamdown's own `CodeBlock` with the
 * same props, so plain snippets look as they always did; a header → the shared `Cell` (U1). Not yet registered in
 * `MessageAssistant.plugins`: streamdown's default path also mounts the copy/download controls as `CodeBlock`
 * children, so a bare `CodeBlock` here would differ until U1 passes them through.
 */
export function CellFence({ code, language, isIncomplete }: CustomRendererProps) {
  if (cellOf(language, code) !== null) return null
  return <CodeBlock code={code} language={language} isIncomplete={isIncomplete} />
}
