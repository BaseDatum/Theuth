import { GitFork, Pencil } from "lucide-react";
import { memo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { useApi } from "@/lib/rpc";
import { useSessionField } from "@/store/sessions";
import { Attachments } from "./Attachments";
import { ActionButton, rewind, useFork } from "./MessageAssistant";

/** The bubble as a textarea: Enter sends, Shift+Enter breaks the line, Escape cancels. Mounted only while editing, so `text` seeds it once. */
function EditBubble({
  text,
  onSend,
  onCancel,
}: {
  text: string;
  onSend: (text: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(text);
  const send = () => {
    const t = value.trim();
    if (t) onSend(t);
  };
  return (
    <>
      <Textarea
        autoFocus
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel();
          else if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
          }
        }}
        className="max-w-[70%] min-h-0 rounded-[22px] border-0 bg-layer-3 px-4 py-2.5 text-base shadow-none whitespace-pre-wrap break-words md:text-base"
      />
      <div className="flex gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" onClick={send}>
          Send
        </Button>
      </div>
    </>
  );
}

/** Right-aligned bubble; a `◇ skill` chip above the words when the input was a skill injection. `turn` is set only on the message that opened a turn (steers carry none). */
export const MessageUser = memo(function MessageUser({
  text,
  skill,
  files,
  session,
  turn,
}: {
  text: string;
  skill?: string;
  files?: string[];
  session: string;
  turn?: number;
}) {
  const api = useApi();
  const running = useSessionField(session, "running");
  const fork = useFork(session);
  const [editing, setEditing] = useState(false);
  return (
    <div className="group flex flex-col items-end gap-2 py-2">
      {skill && (
        <span className="rounded-full bg-layer-2 px-3 py-1 text-sm text-fg-secondary">
          ◇ {skill}
        </span>
      )}
      {files && <Attachments files={files} className="justify-end" />}
      {editing && turn != null ? (
        <EditBubble
          text={text}
          onCancel={() => setEditing(false)}
          onSend={(t) => {
            setEditing(false);
            rewind(session, turn, () => api.edit_turn(session, turn, t, []));
          }}
        />
      ) : (
        text && (
          <div className="max-w-[70%] rounded-[22px] bg-layer-3 px-4 py-2.5 whitespace-pre-wrap break-words">
            {text}
          </div>
        )
      )}
      {turn != null && !running && !editing && (
        <div className="-mr-2 flex justify-end opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          <ActionButton
            icon={Pencil}
            label="Edit"
            onClick={() => setEditing(true)}
          />
          <ActionButton
            icon={GitFork}
            label="Fork from here"
            onClick={() => void fork(turn)}
          />
        </div>
      )}
    </div>
  );
});
