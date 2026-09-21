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

/** Build the schema around a frontmatter spec whose config is {@link frontmatterConfig}. */
export function createDocumentSchema(
  frontmatter: BlockSpec<"frontmatter", typeof frontmatterConfig.propSchema, "none">,
) {
  return BlockNoteSchema.create({
    blockSpecs: { ...keptBlockSpecs, frontmatter },
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
