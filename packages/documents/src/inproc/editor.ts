/**
 * The headless editor the documents helper converts with: the four `ServerBlockNoteEditor` calls `convert.ts`
 * makes, built on `@blocknote/core` directly with linkedom (`dom.ts`) as the DOM and without React (the
 * conversions never need a React context). Runtime-neutral, so it bundles into `t` (scripts/build-inproc.ts).
 */
import {
  type Block,
  BlockNoteEditor,
  type BlockNoteEditorOptions,
  type BlockSchema,
  type InlineContentSchema,
  type PartialBlock,
  type StyleSchema,
  blocksToMarkdown,
} from "@blocknote/core";
import { blocksToYXmlFragment, yXmlFragmentToBlocks } from "@blocknote/core/yjs";
import type * as Y from "yjs";
import { domWindow, type DomWindow } from "./dom.ts";

/** The two globals BlockNote's block renderers and HTML⇄markdown code read. */
type DomGlobals = { window?: unknown; document?: unknown };

export class ServerBlockNoteEditor<B extends BlockSchema, I extends InlineContentSchema, S extends StyleSchema> {
  readonly editor: BlockNoteEditor<B, I, S>;
  private readonly window: DomWindow = domWindow();

  static create<B extends BlockSchema, I extends InlineContentSchema, S extends StyleSchema>(options: Partial<BlockNoteEditorOptions<B, I, S>> = {}) {
    return new ServerBlockNoteEditor<B, I, S>(options);
  }

  protected constructor(options: Partial<BlockNoteEditorOptions<B, I, S>>) {
    // `create`'s overloads infer the default schema from a `Partial` options object; the options carry B/I/S.
    this.editor = BlockNoteEditor.create(options) as unknown as BlockNoteEditor<B, I, S>;
  }

  /** `document`/`window` are this DOM's only while `fn` runs (BlockNote's HTML code reads the globals). */
  private async withDom<T>(fn: () => T | Promise<T>): Promise<T> {
    const g = globalThis as DomGlobals;
    const prevWindow = g.window;
    const prevDocument = g.document;
    g.window = this.window;
    g.document = this.window.document;
    try {
      return await fn();
    } finally {
      g.window = prevWindow;
      g.document = prevDocument;
    }
  }

  yXmlFragmentToBlocks(fragment: Y.XmlFragment): Block<B, I, S>[] {
    return yXmlFragmentToBlocks(this.editor, fragment);
  }

  blocksToYXmlFragment(blocks: Block<B, I, S>[], fragment?: Y.XmlFragment) {
    return blocksToYXmlFragment(this.editor, blocks, fragment);
  }

  blocksToMarkdownLossy(blocks: PartialBlock<B, I, S>[]): Promise<string> {
    return this.withDom(() => blocksToMarkdown(blocks, this.editor.pmSchema, this.editor, { document: this.window.document }));
  }

  tryParseMarkdownToBlocks(markdown: string): Promise<Block<B, I, S>[]> {
    return this.withDom(() => this.editor.tryParseMarkdownToBlocks(markdown));
  }
}
