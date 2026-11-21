/**
 * Stand-in for render-only libraries in the trimmed in-process mermaid bundle (`build-inproc-mermaid.ts --trim`):
 * layout engines that only a renderer calls. Plugin registration at chunk load (`cytoscape.use(fcose)`) is a
 * no-op; any other use throws, so a parse path that did reach one shows up as a parity mismatch instead of a
 * silent wrong answer.
 */
function unavailable(): never {
  throw new Error("theuth: render-only library is not bundled in the parse-only mermaid runtime");
}

const stub: unknown = new Proxy(unavailable, {
  get: (_t, key) =>
    key === "__esModule" ? false : key === "default" ? stub : key === "use" ? () => stub : unavailable,
  apply: unavailable,
  construct: unavailable,
});
export default stub;
