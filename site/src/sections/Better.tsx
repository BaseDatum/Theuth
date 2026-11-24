import { Doc } from "./doc";

const ITEMS = [
  {
    name: "dreaming",
    title: "It tidies its memory at night",
    doc: "workspace/knowledge",
    label: "knowledge",
    body: "While the machine is idle, one routine merges and retires stale knowledge, turns your corrections into test cases and proposes edits to skills and standing preferences. An edit stays only if it passes a replay on held-out cases. Nothing is deleted; every night is a git commit you can undo; it spends at most $1 a night unless you raise it.",
  },
  {
    name: "double",
    title: "It learns how you approve",
    doc: "reference/tools-permissions",
    label: "approvals",
    body: "A small local model predicts your answer to each approval before you see it and scores itself against what you pressed. Class by class it earns the right to suggest, then to answer with a grace period; one override drops it back a step. Destructive actions never get past a suggestion, and publishing is never its call.",
  },
  {
    name: "autoresearch",
    title: "It runs the experiments",
    doc: "automation/autoresearch",
    label: "autoresearch",
    body: "Give it a goal, a benchmark that prints a number, a correctness check and the files it may touch. In its own git worktree it tries one idea at a time; the harness runs the benchmark, and a change stays only if it beats the incumbent by more than the measured noise. Evaluators are hashed into custody and a holdout stays hidden. You wake to a chart and changesets to review.",
  },
  {
    name: "Fractal context",
    title: "It keeps the whole thread",
    doc: "workspace/sessions",
    label: "sessions",
    body: "Long chats keep their whole log. The model sees a ledger of goals and decisions, a summary view whose every line points back to its source and a verbatim tail of recent turns, and zooms in when it needs the original. Trims land at planned points, so the provider's prompt cache holds.",
  },
];

export function Better() {
  return (
    <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
      <h2 className="display text-[clamp(44px,5.4vw,76px)]">
        It gets better with use.
      </h2>
      <div className="mt-14 grid gap-x-12 gap-y-12 md:w-[54%] md:grid-cols-2">
        {ITEMS.map((it) => (
          <div key={it.name} className="flex flex-col border-t border-well pt-6">
            <p className="font-mono text-[14px] text-fg">{it.name}</p>
            <h3 className="mt-3 text-[18px] font-medium">{it.title}</h3>
            <p className="mt-3 flex-1 text-[14.5px] leading-relaxed text-fg-2">{it.body}</p>
            <Doc path={it.doc} className="docs-cta mt-4">
              Docs: {it.label} →
            </Doc>
          </div>
        ))}
      </div>
    </section>
  );
}
