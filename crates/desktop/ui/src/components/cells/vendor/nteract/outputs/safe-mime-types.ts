// Vendored from nteract nteract/nteract:src/components/outputs/safe-mime-types.ts (adapted for theuth 171 §16)
// Dropped: `text/latex` (no KaTeX here) and audio/video (no renderer); they fall back to `text/plain`.
/**
 * MIME types safe for main-DOM rendering (no script execution risk).
 * Everything NOT in this set defaults to iframe isolation.
 *
 * This is a security allowlist — when adding new types, verify they
 * cannot execute arbitrary scripts or access the parent DOM.
 */
const MAIN_DOM_SAFE_TYPES: Record<string, true> = {
  // Plain text with ANSI — no script risk
  "text/plain": true,
  // Raster images — <img> tags, no script risk
  "image/png": true,
  "image/jpeg": true,
  "image/gif": true,
  "image/webp": true,
  "image/bmp": true,
  // Structured data — tree viewer, no script risk
  "application/json": true,
  // Rich traceback payloads — nteract's own schema, rendered by TracebackOutput.
  "application/vnd.nteract.traceback+json": true,
};

/** Check if a MIME type can safely render in the main DOM (no iframe needed). */
export function isSafeForMainDom(mimeType: string): boolean {
  return MAIN_DOM_SAFE_TYPES[mimeType] === true;
}
