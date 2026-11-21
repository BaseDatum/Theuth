/**
 * Stand-in for render-only libraries in the trimmed in-process mermaid bundle (`build-inproc-mermaid.ts --trim`):
 * layout engines that only a renderer calls. Plugin registration at chunk load (`cytoscape.use(fcose)`) is a
 * no-op; any other use throws a `RenderOnlyUnavailable`, which `mermaid-core.ts` `answer` rethrows instead of
 * reporting a syntax error: the request then fails as a protocol error and the reply fence fails open, rather
 * than a valid diagram being reported invalid (review m2). The parity corpus catches such a path as a mismatch.
 */
import { RENDER_ONLY_UNAVAILABLE } from "../mermaid-core.ts";

class RenderOnlyUnavailable extends Error {
  readonly [RENDER_ONLY_UNAVAILABLE] = true;
  constructor() {
    super("theuth: render-only library is not bundled in the parse-only mermaid runtime");
    this.name = "RenderOnlyUnavailable";
  }
}

function unavailable(): never {
  throw new RenderOnlyUnavailable();
}

const stub: unknown = new Proxy(unavailable, {
  get: (_t, key) =>
    key === "__esModule" ? false : key === "default" ? stub : key === "use" ? () => stub : unavailable,
  apply: unavailable,
  construct: unavailable,
});
export default stub;
