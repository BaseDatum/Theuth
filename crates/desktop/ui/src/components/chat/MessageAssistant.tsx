import { code } from '@streamdown/code'
import { createMermaidPlugin } from '@streamdown/mermaid'
import { Check, Copy } from 'lucide-react'
import { memo, useState } from 'react'
import { type PluginConfig, Streamdown } from 'streamdown'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { JsonRenderFence } from './surface/JsonRenderFence'

// Module scope: a new object per render would re-initialise the highlighter.
// Mermaid security (design 30 §11) goes through the plugin's own `config` (it spreads it into `mermaid.initialize`);
// `htmlLabels` is set both globally and per-flowchart because the flowchart sub-config defaults it to true.
// `json-render` fences use streamdown's `plugins.renderers` (matched by language before the built-in code block),
// so no `components.code` override is needed and every other language keeps the `@streamdown/code` renderer.
const plugins: PluginConfig = {
  code,
  mermaid: createMermaidPlugin({
    config: { securityLevel: 'strict', htmlLabels: false, flowchart: { htmlLabels: false }, suppressErrorRendering: true, maxEdges: 1000 },
  }),
  renderers: [{ language: 'json-render', component: JsonRenderFence }],
}
const shikiTheme: ['github-light', 'github-dark'] = ['github-light', 'github-dark']

/** The last fenced code block's body, or `null`. */
export function lastCode(text: string): string | null {
  const blocks = [...text.matchAll(/```[^\n]*\n([\s\S]*?)```/g)]
  return blocks.at(-1)?.[1] ?? null
}

/** Copy `text` and show a check for two seconds. 44 px icon button (ChatGPT's action bar). */
export function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }) {
  const [done, setDone] = useState(false)
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          className="size-11 rounded-lg text-fg-secondary"
          aria-label={label}
          onClick={() => {
            void navigator.clipboard.writeText(text)
            setDone(true)
            setTimeout(() => setDone(false), 2000)
          }}
        >
          {done ? <Check /> : <Copy />}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/** Markdown at column width; `open` while the reply streams (only the last entry ever is). */
export const MessageAssistant = memo(function MessageAssistant({ text, open }: { text: string; open: boolean }) {
  return (
    <div className="py-2">
      <Streamdown
        mode={open ? undefined : 'static'}
        isAnimating={open}
        plugins={plugins}
        shikiTheme={shikiTheme}
        className="[&_h1]:mb-2 [&_h1]:text-2xl [&_h1]:leading-8 [&_h1]:font-semibold [&_h2]:mb-2 [&_h2]:mt-6 [&_h2]:text-xl [&_h2]:font-semibold [&_h3]:mt-4 [&_h3]:font-semibold [&_p]:my-1 [&_p]:leading-[26px] [&_li]:pt-3 [&_li]:leading-[26px] [&_a]:underline [&_a]:underline-offset-2 [&_pre]:rounded-2xl [&_table_th]:font-semibold"
      >
        {text}
      </Streamdown>
      {!open && (
        <div className="-ml-2 flex">
          <CopyButton text={text} />
        </div>
      )}
    </div>
  )
})
