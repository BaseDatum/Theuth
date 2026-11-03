// Vendored from nteract nteract/elements:registry/cell/ExecutionStatus.tsx (adapted for theuth 171 §16)
// 171 §11.8: the blue/red palette → `--status-info-*` / `--status-danger-*` tokens.
import type React from "react";
import { Badge } from "@/components/ui/badge";

interface ExecutionStatusProps {
  executionState: string;
}

export const ExecutionStatus: React.FC<ExecutionStatusProps> = ({ executionState }) => {
  switch (executionState) {
    case "idle":
      return null;
    case "queued":
      return (
        <Badge data-slot="execution-status" variant="secondary" className="h-5 text-xs">
          Queued
        </Badge>
      );
    case "running":
      return (
        <Badge
          data-slot="execution-status"
          variant="outline"
          className="h-5 border-info-border bg-info-bg text-xs text-info"
        >
          <div className="mr-1 h-2 w-2 animate-spin rounded-full border border-info border-t-transparent"></div>
          Running
        </Badge>
      );
    case "error":
      return (
        <Badge
          data-slot="execution-status"
          variant="outline"
          className="h-5 border-danger-border bg-danger-bg text-xs text-danger"
        >
          Error
        </Badge>
      );
    default:
      return null;
  }
};
