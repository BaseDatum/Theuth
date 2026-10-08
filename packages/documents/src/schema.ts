/**
 * The one block schema every theuth document uses (design 59 §9.2, §4.5): BlockNote's
 * defaults minus the media blocks (image/video/audio/file — attachments live elsewhere)
 * plus the custom `frontmatter` block a knowledge document carries first.
 *
 * Imported by the helper (`helper.ts`, headless) and by the desktop UI, so this file
 * must stay free of Node-only and React imports. The frontmatter block's *config*
 * (type, props, content) is shared; its *render* is not: the helper renders a bare
 * `<div>`, the UI a React form (`createReactBlockSpec(frontmatterConfig, …)`). Both
 * produce the same ProseMirror schema, which is all Yjs and the converters care about.
 */
import {
  type BlockNoDefaults,
  BlockNoteSchema,
  type BlockSpec,
  type PartialBlockNoDefaults,
  createBlockConfig,
  createBlockSpec,
  defaultBlockSpecs,
  defaultInlineContentSpecs,
  defaultStyleSpecs,
} from "@blocknote/core";

/** Name of the `Y.XmlFragment` a document's blocks live in (59 §4.2 `fragment`). */
export const FRAGMENT = "document-store";

/**
 * The link gate of every editor over this schema (design 71 §2.3): BlockNote drops hrefs outside its URI allowlist
 * at import, paste and export, which would strip a board's `code:` anchors on `doc_insert`. Its default list plus
 * `code:` — `code:<path>[#L…]` is a root-relative reference the board reader resolves, never navigation — and
 * `theuth:` (144 §6: citations `theuth://notes/<id>#<block>` must survive import, paste and export); relative
 * hrefs (no scheme) stay allowed as before. (`isAllowedUri` is typed but not exported by 0.54.2's runtime.)
 */
export const links = {
  isValidLink: (href: string) =>
    !/^[a-z][a-z0-9+.-]*:/i.test(href) || /^(?:https?|ftps?|mailto|tel|callto|sms|cid|xmpp|code|theuth):/i.test(href),
};

/** The frontmatter block's config: one prop, the YAML header text (authoritative). */
export const frontmatterConfig = createBlockConfig(() => ({
  type: "frontmatter" as const,
  propSchema: { yaml: { default: "" } },
  content: "none" as const,
}))();

/** Headless frontmatter spec (no visual); the UI substitutes its React implementation. */
export const frontmatterSpec = createBlockSpec(frontmatterConfig, {
  meta: { selectable: false, isolating: true },
  render: () => {
    const dom = document.createElement("div");
    dom.dataset.frontmatter = "";
    return { dom };
  },
  // Never part of the exported markdown: the helper writes the YAML header itself (§4.5).
  toExternalHTML: () => ({ dom: document.createElement("span") }),
})();

/** Every default block the schema keeps (59 §9.2); `toggleListItem` allowed, exports as a bullet. */
const {
  audio: _audio,
  file: _file,
  image: _image,
  video: _video,
  ...keptBlockSpecs
} = defaultBlockSpecs;

/**
 * Build the schema around a frontmatter spec whose config is {@link frontmatterConfig}. `codeBlock` may be
 * substituted the same way (UI: languages table + diagram render), provided its config is the default's.
 */
export function createDocumentSchema(
  frontmatter: BlockSpec<"frontmatter", typeof frontmatterConfig.propSchema, "none">,
  codeBlock: BlockSpec<"codeBlock", { language: { default: string } }, "plain"> = keptBlockSpecs.codeBlock,
) {
  return BlockNoteSchema.create({
    blockSpecs: { ...keptBlockSpecs, codeBlock, frontmatter },
    inlineContentSpecs: defaultInlineContentSpecs,
    styleSpecs: defaultStyleSpecs,
  });
}

/** The schema with the headless frontmatter block (the helper's; also the type source). */
export const schema = createDocumentSchema(frontmatterSpec);

export type DocumentSchema = typeof schema;
export type DocBlock = BlockNoDefaults<
  DocumentSchema["blockSchema"],
  DocumentSchema["inlineContentSchema"],
  DocumentSchema["styleSchema"]
>;
export type PartialDocBlock = PartialBlockNoDefaults<
  DocumentSchema["blockSchema"],
  DocumentSchema["inlineContentSchema"],
  DocumentSchema["styleSchema"]
>;

/** Block types the slash menu offers (59 §9.3): the schema minus `frontmatter`. */
export const SLASH_MENU_BLOCKS = [
  "paragraph",
  "heading",
  "bulletListItem",
  "numberedListItem",
  "checkListItem",
  "toggleListItem",
  "codeBlock",
  "table",
  "quote",
  "divider",
] as const;
