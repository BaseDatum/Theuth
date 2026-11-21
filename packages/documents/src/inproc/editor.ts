/**
 * SPIKE (in-process documents helper): a drop-in for `@blocknote/server-util`'s `ServerBlockNoteEditor`
 * restricted to the four calls `convert.ts` makes, with the DOM pluggable instead of jsdom and without
 * React (server-util only needs React for `withReactContext`, which the helper never calls). Swapped in
 * by module aliasing — `test-inproc/preload.ts` under `bun test`, the resolver plugin in
 * `scripts/build-inproc.ts` for the embedded bundle — so `convert.ts` stays byte-identical.
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
    this.editor = BlockNoteEditor.create(options);
  }

  /** Same contract as server-util's `_withJSDOM`: `document`/`window` are the DOM's only while `fn` runs. */
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
