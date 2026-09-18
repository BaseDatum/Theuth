import type {
  ComponentGroup,
  ComponentPromptSpec,
  LibraryJSONSchema,
} from "@openuidev/lang-core";

/** Shape of `generated/library.json`; produced by `scripts/generate.ts`, consumed by the helper and the renderer adapter. */
export interface GeneratedLibrary {
  /** sha256 of pins + patches + root + components + schema + reactive + limits. Compatibility hash. */
  hash: string;
  /** Exact `@openuidev/*` versions from package.json. */
  pins: Record<string, string>;
  /** sha256 per in-tree patch file. */
  patches: Record<string, string>;
  limits: {
    sourceBytes: number;
    statements: number;
    depth: number;
    expandedNodes: number;
    evalOps: number;
    evalNodes: number;
  };
  root: string;
  /** Ordered component names (order participates in the hash). */
  components: string[];
  componentGroups: ComponentGroup[];
  /** Upstream `toSpec().components`: named positional signatures and descriptions. */
  signatures: Record<string, ComponentPromptSpec>;
  /** Props carrying upstream's `reactive()` marker, which JSON Schema cannot express. */
  reactive: Record<string, string[]>;
  /** Upstream `toJSONSchema()`; `$defs` keyed by component name, property order = positional order. */
  schema: LibraryJSONSchema;
}
