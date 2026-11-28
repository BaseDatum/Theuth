import type { ComponentRenderer, Library } from '@openuidev/react-lang'
import { library } from '@theuth/apps'
import type { AppRendererProps } from '@theuth/apps/renderer'
import { createElement } from 'react'
import { copyToClipboard } from '@/components/apps/openui-actions'
import { notify as toast } from '@/lib/notify'
import {
  DeckMarkdown,
  isAllowedDeckImageUrl,
  isSafeDeckExternalUrl,
} from './deck-markdown'

/** Rendered instead of an `<input type="password">` inside Deck OpenUI sections. */
export const DECK_MASKED_REFUSED = 'A Deck cannot ask for a password here.'

function BlockedImageNotice({ alt, label }: { alt?: unknown; label?: string }) {
  const text = typeof alt === 'string' && alt.trim() ? `: ${alt.trim()}` : ''
  return (
    <span
      data-deck-blocked-image
      className="inline-block rounded bg-layer-2 px-1.5 py-0.5 text-[11px] text-fg-tertiary"
    >
      [{label ?? 'blocked image'}
      {text}]
    </span>
  )
}

const masklessInput =
  (input: ComponentRenderer<{ type?: string }>): ComponentRenderer<{ type?: string }> =>
  (render) =>
    render.props.type === 'password' ? (
      <p role="alert" data-deck-masked-refused className="text-xs text-destructive">
        {DECK_MASKED_REFUSED}
      </p>
    ) : (
      createElement(input, render)
    )

const safeMarkdownRenderer: ComponentRenderer<{ textMarkdown?: unknown }> = ({ props }) => (
  <DeckMarkdown>{typeof props.textMarkdown === 'string' ? props.textMarkdown : ''}</DeckMarkdown>
)

const safeSingleImage =
  (orig: ComponentRenderer<Record<string, unknown>>): ComponentRenderer<Record<string, unknown>> =>
  (render) => {
    const src = render.props.src
    if (!isAllowedDeckImageUrl(src)) {
      return <BlockedImageNotice alt={render.props.alt} />
    }
    return createElement(orig, {
      ...render,
      props: { ...render.props, src: src.trim() },
    })
  }

const safeImageGallery =
  (orig: ComponentRenderer<Record<string, unknown>>): ComponentRenderer<Record<string, unknown>> =>
  (render) => {
    const rawImages = Array.isArray(render.props.images) ? render.props.images : []
    const allowed = rawImages.filter(
      (item): item is Record<string, unknown> =>
        item !== null &&
        typeof item === 'object' &&
        isAllowedDeckImageUrl((item as Record<string, unknown>).src),
    )
    if (allowed.length === 0) {
      return <BlockedImageNotice label="blocked image gallery" />
    }
    return createElement(orig, {
      ...render,
      props: {
        ...render.props,
        images: allowed.map((img) => ({ ...img, src: (img.src as string).trim() })),
      },
    })
  }

const safeImageText =
  (orig: ComponentRenderer<Record<string, unknown>>): ComponentRenderer<Record<string, unknown>> =>
  (render) => {
    const src = render.props.src
    if (isAllowedDeckImageUrl(src)) {
      return createElement(orig, {
        ...render,
        props: { ...render.props, src: src.trim() },
      })
    }
    const heading =
      typeof render.props.title === 'string'
        ? render.props.title
        : typeof render.props.text === 'string'
          ? render.props.text
          : ''
    const subtitle = typeof render.props.subtitle === 'string' ? render.props.subtitle : ''
    return (
      <div className="space-y-0.5 text-xs">
        <BlockedImageNotice alt={render.props.alt} />
        {heading && <div className="font-medium text-fg-primary">{heading}</div>}
        {subtitle && <div className="text-fg-tertiary">{subtitle}</div>}
      </div>
    )
  }

function scrubContextCardProps(props: Record<string, unknown>): Record<string, unknown> {
  if (!('bgImageSrc' in props) || props.bgImageSrc === undefined) return props
  if (isAllowedDeckImageUrl(props.bgImageSrc)) {
    return { ...props, bgImageSrc: props.bgImageSrc.trim() }
  }
  const next = { ...props }
  delete next.bgImageSrc
  delete next.bgImageAlt
  return next
}

function scrubListItemProps(props: Record<string, unknown>): Record<string, unknown> {
  const img = props.image
  if (!img || typeof img !== 'object') return props
  const src = (img as Record<string, unknown>).src
  if (isAllowedDeckImageUrl(src)) {
    return { ...props, image: { ...(img as Record<string, unknown>), src: src.trim() } }
  }
  const next = { ...props }
  delete next.image
  return next
}

function scrubElementNode(
  node: unknown,
  scrubber: (props: Record<string, unknown>) => Record<string, unknown>,
): unknown {
  if (!node || typeof node !== 'object') return node
  const rec = node as Record<string, unknown>
  if (rec.props && typeof rec.props === 'object' && !Array.isArray(rec.props)) {
    return { ...rec, props: scrubber(rec.props as Record<string, unknown>) }
  }
  return scrubber(rec)
}

const safeContextCardItem =
  (orig: ComponentRenderer<Record<string, unknown>>): ComponentRenderer<Record<string, unknown>> =>
  (render) =>
    createElement(orig, {
      ...render,
      props: scrubContextCardProps(render.props),
    })

const safeContextCardBlock =
  (orig: ComponentRenderer<Record<string, unknown>>): ComponentRenderer<Record<string, unknown>> =>
  (render) => {
    const items = Array.isArray(render.props.items)
      ? render.props.items.map((item) => scrubElementNode(item, scrubContextCardProps))
      : render.props.items
    return createElement(orig, {
      ...render,
      props: { ...render.props, items },
    })
  }

const safeListItem =
  (orig: ComponentRenderer<Record<string, unknown>>): ComponentRenderer<Record<string, unknown>> =>
  (render) =>
    createElement(orig, {
      ...render,
      props: scrubListItemProps(render.props),
    })

const safeListBlock =
  (orig: ComponentRenderer<Record<string, unknown>>): ComponentRenderer<Record<string, unknown>> =>
  (render) => {
    const rawItems = Array.isArray(render.props.items) ? render.props.items : []
    let hadBlocked = false
    const scrubbedItems = rawItems.map((item) => {
      const next = scrubElementNode(item, scrubListItemProps)
      if (next !== item) hadBlocked = true
      return next
    })
    const nextProps: Record<string, unknown> = { ...render.props, items: scrubbedItems }
    if (hadBlocked && nextProps.variant === 'image') {
      nextProps.variant = 'number'
    }
    return createElement(orig, { ...render, props: nextProps })
  }

/**
 * Restricted OpenUI component library for Decks:
 * - Refuses `Input` with `type="password"`
 * - Routes `MarkDownRenderer` through `DeckMarkdown` (`mode="static"`, `skipHtml`, no raw HTML, no external images)
 * - Blocks dynamic/external `http(s)` images across all image-bearing components (`Image`, `ImageBlock`,
 *   `ImageGallery`, `ImageText`, `ImageTextLarge`, `ContextCardItem`, `ContextCardBlock`, `ListItem`, `ListBlock`)
 */
export function restrictForDecks(base: Library): Library {
  if (!base.extend) {
    throw new Error('[decks] the apps library cannot be extended')
  }
  const c = base.components
  const overrides: Array<Record<string, unknown>> = []

  if (!c.Input || !c.MarkDownRenderer) {
    throw new Error('[decks] the apps library lacks Input or MarkDownRenderer')
  }
  overrides.push(
    { ...c.Input, component: masklessInput(c.Input.component as ComponentRenderer<{ type?: string }>) },
    { ...c.MarkDownRenderer, component: safeMarkdownRenderer },
  )

  const wrapIfPresent = (
    name: string,
    wrapper: (
      orig: ComponentRenderer<Record<string, unknown>>,
    ) => ComponentRenderer<Record<string, unknown>>,
  ) => {
    const def = c[name]
    if (def) {
      overrides.push({
        ...def,
        component: wrapper(def.component as ComponentRenderer<Record<string, unknown>>),
      })
    }
  }

  wrapIfPresent('Image', safeSingleImage)
  wrapIfPresent('ImageBlock', safeSingleImage)
  wrapIfPresent('ImageGallery', safeImageGallery)
  wrapIfPresent('ImageText', safeImageText)
  wrapIfPresent('ImageTextLarge', safeImageText)
  wrapIfPresent('ContextCardItem', safeContextCardItem)
  wrapIfPresent('ContextCardBlock', safeContextCardBlock)
  wrapIfPresent('ListItem', safeListItem)
  wrapIfPresent('ListBlock', safeListBlock)

  return base.extend({
    components: { override: overrides as never },
  }) as unknown as Library
}

/** Singleton Deck OpenUI library (memoized by identity inside `@openuidev/react-lang`). */
export const deckOpenUILibrary: Library = restrictForDecks(library)

/**
 * Restricted OpenUI action handler for Decks:
 * - `open_url`: allows ONLY `http://` or `https://` URLs with `noopener,noreferrer`
 * - `CopyToClipboard`: copies string text locally
 * - `continue_conversation` (`@ToAssistant`): strictly inert in Decks (no chat draft, no navigation, no RPC)
 */
export function handleDeckOpenUIAction(
  ev: Parameters<NonNullable<AppRendererProps['onAction']>>[0],
  openUrl: (url: string) => void = (u) => {
    window.open(u, '_blank', 'noopener,noreferrer')
  },
): boolean {
  if (ev.type === 'open_url') {
    const url = ev.params?.url
    if (isSafeDeckExternalUrl(url)) {
      openUrl(url.trim())
      return true
    }
    toast.error('Only http(s) links can open from a Deck')
    return false
  }
  if (ev.type === 'CopyToClipboard') {
    copyToClipboard(ev.params ?? {})
    return true
  }
  if (ev.type === 'continue_conversation') {
    // @ToAssistant is strictly inert in Deck OpenUI sections.
    return false
  }
  return false
}
