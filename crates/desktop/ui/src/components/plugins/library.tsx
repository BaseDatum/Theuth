// 216 §9.4: the OpenUI library a plugin surface renders with — the apps' curated `library` with two components
// replaced, as defence in depth behind the host's checks. `Input` never draws a masked field: the host refuses a
// `"password"` argument in the validated source, and a source that slips past it gets a notice, not the field.
// `MarkDownRenderer` goes through the same pipeline as a `markdown` surface: raw HTML skipped, never parsed.
import type { ComponentRenderer, Library } from '@openuidev/react-lang'
import { library } from '@theuth/apps'
import { createElement } from 'react'
import { Streamdown, defaultRehypePlugins } from 'streamdown'
import { markdownComponents, rehypePlugins } from '@/components/chat/MessageAssistant'

/** The chat's sanitize/harden steps without `raw`: HTML in a plugin's markdown stays unparsed, and `skipHtml` drops it. */
const MARKDOWN_REHYPE = rehypePlugins.filter((p) => p !== defaultRehypePlugins.raw)

/** A plugin's markdown, from a `markdown` surface or an OpenUI `MarkDownRenderer`. */
export function PluginMarkdown({ children }: { children: string }) {
  return (
    <Streamdown mode="static" skipHtml rehypePlugins={MARKDOWN_REHYPE} components={markdownComponents} className="[&_a]:underline [&_a]:underline-offset-2 [&_p]:my-1">
      {children}
    </Streamdown>
  )
}

/** What a masked `Input` draws instead of the field. */
export const MASKED_REFUSED = 'A plugin cannot ask for a password here.'

const maskless =
  (input: ComponentRenderer<{ type?: string }>): ComponentRenderer<{ type?: string }> =>
  (render) =>
    render.props.type === 'password' ? (
      <p role="alert" className="text-[13px] text-destructive">
        {MASKED_REFUSED}
      </p>
    ) : (
      createElement(input, render)
    )

const MarkdownBlock: ComponentRenderer<{ textMarkdown?: unknown }> = ({ props }) => (
  <PluginMarkdown>{typeof props.textMarkdown === 'string' ? props.textMarkdown : ''}</PluginMarkdown>
)

/** `base` with `Input` and `MarkDownRenderer` replaced; same schemas, so the host's validation still describes it. */
export function restrictForPlugins(base: Library): Library {
  const { Input, MarkDownRenderer } = base.components
  if (!base.extend || !Input || !MarkDownRenderer) throw new Error('[plugins] the apps library lacks Input or MarkDownRenderer')
  return base.extend({
    components: { override: [{ ...Input, component: maskless(Input.component) }, { ...MarkDownRenderer, component: MarkdownBlock }] },
  }) as unknown as Library
}

/** Built once: the renderer's parser memoizes by library identity. */
export const pluginLibrary: Library = restrictForPlugins(library)
