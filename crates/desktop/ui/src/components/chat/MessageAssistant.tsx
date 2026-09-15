import { code } from "@streamdown/code";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Check, Copy, GitFork, type LucideIcon, RefreshCw } from "lucide-react";
import { memo, useState } from "react";
import { toast } from "sonner";
import { Streamdown } from "streamdown";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { queryKeys, useApi } from "@/lib/rpc";
import { useSessionField, useSessions } from "@/store/sessions";

// Module scope: a new object per render would re-initialise the highlighter.
const plugins = { code };
const shikiTheme: ["github-light", "github-dark"] = [
  "github-light",
  "github-dark",
];

/** The last fenced code block's body, or `null`. */
export function lastCode(text: string): string | null {
  const blocks = [...text.matchAll(/```[^\n]*\n([\s\S]*?)```/g)];
  return blocks.at(-1)?.[1] ?? null;
}

/** 44 px ghost icon button with a tooltip (ChatGPT's action bar); `done` swaps the icon for a check. */
export function ActionButton({
  icon: Icon,
  label,
  onClick,
  done,
}: {
  icon: LucideIcon;
  label: string;
  onClick: () => void;
  done?: boolean;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          className="size-11 rounded-lg text-fg-secondary"
          aria-label={label}
          onClick={onClick}
        >
          {done ? <Check /> : <Icon />}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** Copy `text` and show a check for two seconds. */
export function CopyButton({
  text,
  label = "Copy",
}: {
  text: string;
  label?: string;
}) {
  const [done, setDone] = useState(false);
  return (
    <ActionButton
      icon={Copy}
      label={label}
      done={done}
      onClick={() => {
        void navigator.clipboard.writeText(text);
        setDone(true);
        setTimeout(() => setDone(false), 2000);
      }}
    />
  );
}

/** Cut the transcript at `turn` optimistically, then run `rpc`; on failure toast and re-fetch the transcript (29 §3). */
export function rewind(
  session: string,
  turn: number,
  rpc: () => Promise<unknown>,
) {
  useSessions.getState().truncate(session, turn);
  rpc().catch((e: unknown) => {
    toast.error(e instanceof Error ? e.message : String(e));
    window.dispatchEvent(new Event("wth:reload-session"));
  });
}

/** `fork(turn)`: copy `session` through `turn`, refresh the list, open the copy. */
export function useFork(session: string) {
  const api = useApi();
  const qc = useQueryClient();
  const navigate = useNavigate();
  return async (turn: number) => {
    try {
      const id = await api.fork_session(session, turn);
      void qc.invalidateQueries({ queryKey: queryKeys.sessions });
      await navigate({ to: "/c/$session", params: { session: id } });
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };
}

/** Markdown at column width; `open` while the reply streams (only the last entry ever is). `turn` is the opener seq the reply belongs to. */
export const MessageAssistant = memo(function MessageAssistant({
  text,
  open,
  session,
  turn,
}: {
  text: string;
  open: boolean;
  session: string;
  turn?: number;
}) {
  const api = useApi();
  const running = useSessionField(session, "running");
  const fork = useFork(session);
  return (
    <div className="py-2">
      <Streamdown
        mode={open ? undefined : "static"}
        isAnimating={open}
        plugins={plugins}
        shikiTheme={shikiTheme}
        className="[&_h1]:mb-2 [&_h1]:text-2xl [&_h1]:leading-8 [&_h1]:font-semibold [&_h2]:mb-2 [&_h2]:mt-6 [&_h2]:text-xl [&_h2]:font-semibold [&_h3]:mt-4 [&_h3]:font-semibold [&_p]:my-1 [&_p]:leading-[26px] [&_li]:pt-3 [&_li]:leading-[26px] [&_a]:underline [&_a]:underline-offset-2 [&_pre]:rounded-2xl [&_table_th]:font-semibold"
      >
        {text}
      </Streamdown>
      {!open && (
        <div className="-ml-2 flex">
          <CopyButton text={text} />
          {turn != null && !running && (
            <>
              <ActionButton
                icon={RefreshCw}
                label="Regenerate"
                onClick={() =>
                  rewind(session, turn, () => api.regenerate(session, turn))
                }
              />
              <ActionButton
                icon={GitFork}
                label="Fork from here"
                onClick={() => void fork(turn)}
              />
            </>
          )}
        </div>
      )}
    </div>
  );
});
