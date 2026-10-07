#!/usr/bin/env bun
/**
 * Design 123 §8.3 steering measurement: ask a model, under the real `## Show first, then tell`
 * block, each prompt of `prompts.json` and count which fence/component it draws.
 *
 *   bun scripts/123-steering/run.ts [--model claude-opus-5-5] [--block /path/to/block.txt]
 *       [--out /tmp/dir] [--concurrency 4] [--only grammar|hold] [--validate] [--dry-run]
 *
 * The block defaults to `cargo run -q -p surface --example prompt_block` from this tree (so a
 * `--block` captured from another ref gives the "before" number). Needs ANTHROPIC_API_KEY.
 * A grammar prompt is right when a json-render element's `type` is in its `expect`; a hold
 * prompt is a false draw when any element's `type` is one of the nineteen new grammars.
 * `--validate` also runs every reply through the crate's validator (`… prompt_block check`) and
 * reports first-pass validity. Exit 0 when ≥ 16/20 and ≤ 1/10.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";

const HERE = import.meta.dir;
const { values: args } = parseArgs({
  options: {
    model: { type: "string", default: "claude-opus-5-5" },
    block: { type: "string" },
    out: { type: "string" },
    concurrency: { type: "string", default: "4" },
    only: { type: "string" },
    "dry-run": { type: "boolean", default: false },
    validate: { type: "boolean", default: false },
  },
});

type Case = { id: string; group?: string; expect?: string[]; prompt: string };
type Cases = { new_grammars: string[]; grammar: Case[]; hold: Case[] };
const cases: Cases = await Bun.file(join(HERE, "prompts.json")).json();
const NEW: Record<string, true> = Object.fromEntries(cases.new_grammars.map((n) => [n, true]));

const block = args.block
  ? await Bun.file(args.block).text()
  : await Bun.$`cargo run -q -p surface --example prompt_block block`.cwd(join(HERE, "../..")).text();

/** The attended frame around the block (30 §6.1): one line of role, the Guidance sentence, the block. */
const system = [
  "You are theuth, a coding assistant chatting with a developer in a desktop app that renders fenced blocks inline.",
  "Report visually first (see the Show first, then tell section). Answer directly; do not ask clarifying questions.",
  "",
  block,
].join("\n");

const FENCE = /```(json-render|mermaid|openui)\n([\s\S]*?)```/g;

/** What a reply drew: json-render element types, mermaid first words, `openui` for each openui fence. */
function picks(reply: string): string[] {
  const out: string[] = [];
  for (const m of reply.matchAll(FENCE)) {
    const [, lang, body] = m as unknown as [string, string, string];
    if (lang === "json-render") {
      try {
        const v = JSON.parse(body);
        for (const el of Object.values(v.elements ?? {}) as { type?: string }[])
          if (el.type) out.push(el.type);
      } catch {
        out.push("json-render:invalid");
      }
    } else if (lang === "mermaid") {
      out.push(`mermaid:${body.trim().split(/\s/)[0]}`);
    } else out.push("openui");
  }
  return out;
}

async function ask(prompt: string): Promise<string> {
  const res = await fetch("https://api.anthropic.com/v1/messages", {
    method: "POST",
    headers: {
      "x-api-key": process.env.ANTHROPIC_API_KEY ?? "",
      "anthropic-version": "2023-06-01",
      "content-type": "application/json",
    },
    body: JSON.stringify({
      model: args.model,
      max_tokens: 4000,
      system,
      messages: [{ role: "user", content: prompt }],
    }),
  });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  const json = (await res.json()) as { content: { type: string; text?: string }[] };
  return json.content.map((c) => c.text ?? "").join("");
}

async function pool<T, R>(items: T[], n: number, f: (t: T) => Promise<R>): Promise<R[]> {
  const out: R[] = new Array(items.length);
  let i = 0;
  await Promise.all(
    Array.from({ length: n }, async () => {
      for (let k = i++; k < items.length; k = i++) out[k] = await f(items[k]!);
    }),
  );
  return out;
}

type Row = { id: string; group: string; picks: string[]; ok: boolean; errors: string[]; reply: string };

/** Located errors of every json-render fence in `reply`, from the crate's validator (empty when `--validate` is off). */
async function validate(reply: string): Promise<string[]> {
  if (!args.validate) return [];
  const r = await Bun.$`cargo run -q -p surface --example prompt_block check < ${new Response(reply)}`
    .cwd(join(HERE, "../.."))
    .nothrow()
    .text();
  return r.split("\n").filter(Boolean);
}

const run = async (list: Case[], judge: (c: Case, p: string[]) => boolean): Promise<Row[]> =>
  pool(list, Number(args.concurrency), async (c) => {
    const reply = await ask(c.prompt);
    const p = picks(reply);
    const ok = judge(c, p);
    const errors = await validate(reply);
    console.error(`${ok ? "✓" : "✗"} ${c.id.padEnd(16)} ${p.join(", ") || "(prose)"}${errors.length ? ` — ${errors.length} error(s)` : ""}`);
    return { id: c.id, group: c.group ?? "hold", picks: p, ok, errors, reply };
  });

const out = args.out ?? `/tmp/theuth-123-steering-${new Date().toISOString().replace(/[:.]/g, "-")}`;
mkdirSync(out, { recursive: true });
writeFileSync(join(out, "system.md"), system);
if (args["dry-run"]) {
  console.log(JSON.stringify({ model: args.model, block_bytes: block.length, grammar: cases.grammar.length, hold: cases.hold.length, out }, null, 2));
  process.exit(0);
}

const grammar = args.only === "hold" ? [] : await run(cases.grammar, (c, p) => p.some((t) => c.expect!.includes(t)));
const hold = args.only === "grammar" ? [] : await run(cases.hold, (_c, p) => !p.some((t) => NEW[t]));

const right = grammar.filter((r) => r.ok).length;
const falseDraws = hold.filter((r) => !r.ok).length;
const byGroup: Record<string, string> = {};
for (const g of new Set(grammar.map((r) => r.group)))
  byGroup[g] = `${grammar.filter((r) => r.group === g && r.ok).length}/${grammar.filter((r) => r.group === g).length}`;

const all = [...grammar, ...hold];
const summary = {
  model: args.model,
  block_bytes: block.length,
  grammar: { right, total: grammar.length, by_group: byGroup, bar: ">= 16/20" },
  hold: { false_draws: falseDraws, total: hold.length, bar: "<= 1/10" },
  first_pass_valid: args.validate ? `${all.filter((r) => r.errors.length === 0).length}/${all.length}` : "not measured (--validate)",
  rows: all.map(({ reply: _r, ...rest }) => rest),
};
writeFileSync(join(out, "summary.json"), JSON.stringify(summary, null, 2));
for (const r of [...grammar, ...hold]) writeFileSync(join(out, `${r.id}.md`), r.reply);
console.log(JSON.stringify({ ...summary, rows: undefined, out }, null, 2));
const pass = (grammar.length === 0 || right >= 16) && (hold.length === 0 || falseDraws <= 1);
process.exit(pass ? 0 : 1);
