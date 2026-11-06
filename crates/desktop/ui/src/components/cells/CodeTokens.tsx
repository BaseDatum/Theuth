// Owner: U1
import { type CSSProperties, Fragment, useEffect, useState } from 'react'
import type { ThemedToken } from 'shiki'
import { tokensOf } from '@/lib/shiki'

/** Tokens for one `code` in one `lang`; another pair renders plain until its own tokens arrive. */
export type Lit = { code: string; lang: string; tokens: ThemedToken[][] }

/** The render: `lit`'s tokens if they are for this `code` and `lang`, else the plain text. */
export function CodeTokensView({ code, lang, lit }: { code: string; lang: string; lit: Lit | null }) {
  if (lit?.code !== code || lit.lang !== lang) return code
  return lit.tokens.map((line, i) => (
    <Fragment key={i}>
      {i > 0 && '\n'}
      {line.map((t, j) => (
        <span key={j} className="shiki" style={t.htmlStyle as CSSProperties}>
          {t.content}
        </span>
      ))}
    </Fragment>
  ))
}

/** Read-only cell code (the raw `#|` header, the code a run ran) in the editor's shiki colours; plain until loaded. */
export function CodeTokens({ code, lang }: { code: string; lang: string }) {
  const [lit, setLit] = useState<Lit | null>(null)
  useEffect(() => {
    let live = true
    tokensOf(code, lang).then(
      (tokens) => live && setLit({ code, lang, tokens }),
      () => undefined, // no grammar: stays plain
    )
    return () => {
      live = false
    }
  }, [code, lang])
  return <CodeTokensView code={code} lang={lang} lit={lit} />
}
