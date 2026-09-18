/**
 * @wth/apps — OpenUI Lang adapter for wth apps (design 46).
 *
 * Frontend entry: curated `library`, compatibility `libraryHash`, and the shared `check`/`mergeCheck`
 * validator. Pulls the react-ui component graph; load it lazily on first app preview/open.
 * The renderer wrapper lives at `@wth/apps/renderer`; the headless helper imports `./check.ts` only.
 */
export {
  CURATED_COMPONENTS,
  LIBRARY_ROOT,
  buildCuratedLibrary,
  library,
  reactiveProps,
} from "./library.ts";
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
