// Vendored from nteract nteract/nteract:src/components/cell/OutputArea.tsx (adapted for theuth 171 §16)
// Kept: the nbformat output list and `renderOutput`'s per-type dispatch (MediaRouter / AnsiStreamOutput /
// TracebackOutput / AnsiErrorOutput). Dropped: iframe isolation and the sift handoff (HTML goes through the host's
// `renderers`, 171 §8.2), widgets, search highlighting, output wells and focus modes, runtimed anchors, the
// error boundary (nteract's own module), and the rich `output.rich` sibling (our outputs never carry one).
import { useContext } from "react";
import { disclosureKey } from "@/lib/tool-runs";
import { cn } from "@/lib/utils";
import { AnsiErrorOutput, AnsiStreamOutput, OutputDisclosure } from "../outputs/ansi-output";
import { type CustomRenderer, DEFAULT_PRIORITY, MediaRouter } from "../outputs/media-router";
import { classicTracebackToPayload, TracebackOutput } from "../outputs/traceback-output";
import { cellOutputInnerInset } from "./cell-layout";

/** Jupyter output types based on the nbformat spec (nteract's `jupyter-output.ts`, inlined). */
export type JupyterOutput =
  | {
      output_type: "execute_result" | "display_data";
      data: Record<string, unknown>;
      metadata?: Record<string, unknown>;
      execution_count?: number | null;
    }
  | {
      output_type: "stream";
      name: "stdout" | "stderr";
      text: string | string[];
    }
  | {
      output_type: "error";
      ename: string;
      evalue: string;
      traceback: string[];
    };

interface OutputAreaProps {
  /** Array of Jupyter outputs to render. */
  outputs: JupyterOutput[];
  /** Additional CSS classes for the container. */
  className?: string;
  /** `cell-output` sits inside `CellContainer`'s output row; `standalone` carries no inset. */
  layoutInset?: "standalone" | "cell-output";
  /**
   * Custom renderers keyed by MIME type, per output index (theuth: the frame
   * renderers need the output's position for their `callId`).
   */
  renderersFor?: (index: number) => Record<string, CustomRenderer> | undefined;
  /** Custom MIME type priority order. */
  priority?: readonly string[];
}

/**
 * Render a single Jupyter output based on its type.
 */
function renderOutput(
  output: JupyterOutput,
  index: number,
  renderers: Record<string, CustomRenderer> | undefined,
  priority: readonly string[],
) {
  const key = `output-${index}`;

  switch (output.output_type) {
    case "execute_result":
    case "display_data":
      return (
        <MediaRouter
          key={key}
          data={output.data}
          metadata={output.metadata as Record<string, Record<string, unknown> | undefined>}
          renderers={renderers}
          priority={priority}
        />
      );

    case "stream":
      return (
        <AnsiStreamOutput
          key={key}
          text={Array.isArray(output.text) ? output.text.join("") : output.text}
          streamName={output.name}
        />
      );

    case "error": {
      const classicTraceback = classicTracebackToPayload({
        ename: output.ename,
        evalue: output.evalue,
        traceback: output.traceback,
      });
      if (classicTraceback) {
        return <TracebackOutput key={key} data={classicTraceback} />;
      }
      return (
        <AnsiErrorOutput
          key={key}
          ename={output.ename}
          evalue={output.evalue}
          traceback={output.traceback}
        />
      );
    }

    default:
      return null;
  }
}

/**
 * OutputArea renders multiple Jupyter outputs with proper layout.
 */
export function OutputArea({
  outputs,
  className,
  layoutInset = "standalone",
  renderersFor,
  priority = DEFAULT_PRIORITY,
}: OutputAreaProps) {
  const base = useContext(OutputDisclosure);
  if (outputs.length === 0) return null;

  return (
    <div
      data-slot="output-area"
      className={cn("output-area pr-3", layoutInset === "cell-output" && cellOutputInnerInset, className)}
    >
      <div className="space-y-2">
        {outputs.map((output, index) => (
          <div key={index} data-slot="output-item" data-output-index={index}>
            {/* 171F B13: each output's toggles are keyed by its index under the run's key. */}
            <OutputDisclosure.Provider value={disclosureKey(base, index)}>
              {renderOutput(output, index, renderersFor?.(index), priority)}
            </OutputDisclosure.Provider>
          </div>
        ))}
      </div>
    </div>
  );
}
