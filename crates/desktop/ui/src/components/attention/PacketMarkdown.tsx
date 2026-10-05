import type { MouseEvent } from 'react'
import { Streamdown } from 'streamdown'
import { markdownComponents, plugins, shikiTheme } from '@/components/chat/MessageAssistant'
import { FenceContext } from '@/components/chat/surface/JsonRenderFence'
import { openExternal } from './Pointers'

/** The settled, non-streaming case: a broken fence shows its one dim line, never a skeleton. */
const SETTLED: FenceContext = { running: false, hasLaterAssistant: false }

// Same classes as the assistant message (`MessageAssistant.tsx`) so headings, lists and sideways-scrolling tables look alike.
const MD_CLASS =
  '[&_h1]:mb-2 [&_h1]:text-2xl [&_h1]:leading-8 [&_h1]:font-semibold [&_h2]:mb-2 [&_h2]:mt-6 [&_h2]:text-xl [&_h2]:font-semibold [&_h3]:mt-4 [&_h3]:font-semibold [&_p]:my-1 [&_p]:leading-[26px] [&_li]:pt-3 [&_li]:leading-[26px] [&_a]:underline [&_a]:underline-offset-2 [&_pre]:rounded-2xl [&_table]:w-max [&_table]:min-w-full [&_table_th]:font-semibold [&_table_th]:wrap-normal'

/** A packet's `summary` (101 §3) through streamdown with the chat's renderers: ```mermaid, ```json-render and ```openui fences render. */
export function PacketMarkdown({ text }: { text: string }) {
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const href = e.target instanceof Element ? e.target.closest('a')?.getAttribute('href') : null
    if (!href || href.startsWith('#')) return
    e.preventDefault()
    if (/^https?:/.test(href)) openExternal(href)
  }
  return (
    <div onClickCapture={onClick} className="text-sm">
      <FenceContext value={SETTLED}>
        <Streamdown mode="static" plugins={plugins} components={markdownComponents} shikiTheme={shikiTheme} linkSafety={{ enabled: false }} className={MD_CLASS}>
          {text}
        </Streamdown>
      </FenceContext>
    </div>
  )
}
