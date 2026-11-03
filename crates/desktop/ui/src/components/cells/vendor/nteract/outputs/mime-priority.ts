// Vendored from nteract nteract/nteract:src/components/outputs/mime-priority.ts (adapted for theuth 171 §16)
// Dropped (171 §8.2): widgets, plotly, bokeh/panel, vega, geo+json, arrow/parquet, pdf, latex, javascript, audio,
// video — unsupported bundles fall through to `text/plain`.

/**
 * Default MIME type priority order for rendering.
 * Higher priority types are preferred when multiple are available.
 */
export const DEFAULT_PRIORITY = [
  "text/html",
  "image/svg+xml",
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/bmp",
  "text/markdown",
  "application/json",
  "text/plain",
] as const;

export type MimeType = (typeof DEFAULT_PRIORITY)[number] | string;

/** nteract's agent-only preview text; never shown to the user. */
export function isPreviewOnlyMimeType(mimeType: string): boolean {
  return mimeType === "text/llm+plain";
}

/**
 * Select the best MIME type from available data based on priority. A bundle
 * with none of the known types (plotly + nothing else) yields its first key
 * only so the router can show it as text; `text/plain` always wins first.
 */
export function selectMimeType(
  data: Record<string, unknown>,
  priority: readonly string[] = DEFAULT_PRIORITY,
): MimeType | null {
  const availableTypes = Object.keys(data);
  for (const mimeType of priority) {
    if (
      availableTypes.includes(mimeType) &&
      !isPreviewOnlyMimeType(mimeType) &&
      data[mimeType] != null
    ) {
      return mimeType;
    }
  }
  const firstAvailable = availableTypes.find(
    (type) => !isPreviewOnlyMimeType(type) && data[type] != null,
  );
  return firstAvailable || null;
}
