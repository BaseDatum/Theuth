// Vendored from nteract nteract/nteract:src/components/outputs/media-router.tsx (adapted for theuth 171 §16)
// Dropped: the renderer-registry plugins, markdown projection, media-provider context, lazy chunks, and the
// iframe-only renderers (html, svg, markdown, javascript), plus math/audio/video/pdf/widgets (171 §8.2). The host
// supplies the frame-bound MIME types through `renderers`; without one they fall back to the bundle's
// `text/plain`, never to main-DOM HTML.
import { type ReactNode } from "react";
import { AnsiOutput } from "./ansi-output";
import { ImageOutput } from "./image-output";
import { JsonOutput } from "./json-output";
import { TracebackOutput } from "./traceback-output";
import { isSafeForMainDom } from "./safe-mime-types";
import { DEFAULT_PRIORITY, selectMimeType } from "./mime-priority";
export { DEFAULT_PRIORITY, selectMimeType } from "./mime-priority";
export type { MimeType } from "./mime-priority";

interface MediaData {
  [mimeType: string]: unknown;
}

interface MediaMetadata {
  [mimeType: string]:
    | {
        width?: number;
        height?: number;
        [key: string]: unknown;
      }
    | undefined;
}

/**
 * Props passed to custom renderer functions.
 */
export interface RendererProps {
  data: unknown;
  metadata: Record<string, unknown>;
  mimeType: string;
  className?: string;
}

/**
 * Custom renderer function type.
 */
export type CustomRenderer = (props: RendererProps) => ReactNode;

interface MediaRouterProps {
  /**
   * Output data object mapping MIME types to content.
   * e.g., { "text/plain": "Hello", "text/html": "<b>Hello</b>" }
   */
  data: MediaData;
  /**
   * Output metadata object mapping MIME types to their metadata.
   * e.g., { "image/png": { width: 400, height: 300 } }
   */
  metadata?: MediaMetadata;
  /**
   * Custom MIME type priority order. Types listed first are preferred.
   * Defaults to DEFAULT_PRIORITY.
   */
  priority?: readonly string[];
  /**
   * Custom renderers keyed by MIME type. Use this to handle MIME types
   * not supported by the built-in renderers, or to override built-ins.
   */
  renderers?: Record<string, CustomRenderer>;
  /**
   * Custom fallback component when no supported MIME type is found.
   */
  fallback?: ReactNode;
  /**
   * Additional CSS classes passed to the rendered output.
   */
  className?: string;
}

/**
 * MediaRouter component for rendering Jupyter outputs based on MIME type.
 *
 * Automatically selects the best available renderer for the output data,
 * following Jupyter's MIME type priority conventions. Supports custom
 * renderers and priority ordering for platform-specific MIME types.
 */
export function MediaRouter({
  data,
  metadata = {},
  priority = DEFAULT_PRIORITY,
  renderers = {},
  fallback,
  className = "",
}: MediaRouterProps) {
  const mimeType = selectMimeType(data, priority);

  if (!mimeType) {
    return (
      <div data-slot="media-router">
        {fallback ? fallback : <div className="py-2 text-sm text-fg-tertiary">No displayable output</div>}
      </div>
    );
  }

  const content = data[mimeType];
  const mimeMetadata = (metadata[mimeType] || {}) as Record<string, unknown>;

  // Check for custom renderer first
  const customRenderer = renderers[mimeType];
  if (customRenderer) {
    return (
      <div data-slot="media-router" data-mime-type={mimeType}>
        {customRenderer({ data: content, metadata: mimeMetadata, mimeType, className })}
      </div>
    );
  }

  const renderBuiltIn = () => {
    // ISOLATION GUARD: Only types in the safe-list can render in the main DOM.
    // Everything else needs the host's frame renderer; without one, the
    // bundle's plain text stands in (171 §8.2), never the markup itself.
    if (!isSafeForMainDom(mimeType)) {
      const plain = data["text/plain"];
      return plain == null ? null : <AnsiOutput className={className}>{String(plain)}</AnsiOutput>;
    }

    // Images (not SVG)
    if (mimeType.startsWith("image/")) {
      return (
        <ImageOutput
          data={String(content)}
          mediaType={mimeType}
          width={mimeMetadata.width as number | undefined}
          height={mimeMetadata.height as number | undefined}
          className={className}
        />
      );
    }

    // Rich traceback — nteract's own schema, main-DOM React.
    if (mimeType === "application/vnd.nteract.traceback+json") {
      return <TracebackOutput data={content} className={className} />;
    }

    // JSON and structured data
    if (mimeType === "application/json") {
      return (
        <JsonOutput
          data={content}
          collapsed={mimeMetadata.collapsed as boolean | number | undefined}
          className={className}
        />
      );
    }

    // Plain text (may contain ANSI)
    return <AnsiOutput className={className}>{String(content)}</AnsiOutput>;
  };

  return (
    <div data-slot="media-router" data-mime-type={mimeType}>
      {renderBuiltIn()}
    </div>
  );
}

/**
 * Get the selected MIME type for debugging/display purposes.
 */
export function getSelectedMimeType(
  data: MediaData,
  priority: readonly string[] = DEFAULT_PRIORITY,
): string | null {
  return selectMimeType(data, priority);
}
