/**
 * @theuth/apps — OpenUI Lang adapter for theuth apps (design 46).
 *
 * Frontend entry: curated `library`, compatibility `libraryHash`, and the shared `check`/`mergeCheck`
 * validator. Pulls the react-ui component graph; load it lazily on first app preview/open.
 * The renderer wrapper lives at `@theuth/apps/renderer`; the in-process bundles import `./check.ts` (inproc.ts) and
 * `./mermaid-core.ts` (inproc-mermaid.ts) only.
 */
export {
  CURATED_COMPONENTS,
  LIBRARY_ROOT,
  buildCuratedLibrary,
  graphProps,
  graphText,
  library,
  reactiveProps,
  setGraphRenderer,
  setDiagramRenderer,
  diagramRenderer,
} from "./library.ts";
export type { DiagramHostProps, GraphHostProps, GraphValue } from "./library.ts";
export { DIAGRAMS, diagramText } from "./diagrams.ts";
export { check, generatedLibrary, libraryHash, mergeCheck } from "./check.ts";
export type { GeneratedLibrary } from "./generated-types.ts";
export type {
  Binding,
  CheckResult,
  Diagnostic,
  DiagnosticCode,
  ParserErrorResponse,
  ParserOp,
  ParserRequest,
  ParserResponse,
  StatementInfo,
  StatementKind,
  ToolSnapshot,
} from "./protocol.ts";
export { LIMITS, LimitError, createEvalBudget } from "@openuidev/lang-core";
export type { EvalBudget, LimitKind } from "@openuidev/lang-core";
