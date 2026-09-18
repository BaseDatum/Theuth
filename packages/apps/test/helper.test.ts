import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { libraryHash } from "../src/check.ts";
import type { ParserErrorResponse, ParserResponse } from "../src/protocol.ts";

const entry = join(import.meta.dir, "..", "src", "helper.ts");
const env = {
  ...process.env,
  OPENUI_TELEMETRY_DISABLED: "1",
  DO_NOT_TRACK: "1",
};

async function spawnHelper(
  stdin: string,
  ...args: string[]
): Promise<{ code: number; stdout: string }> {
  const proc = Bun.spawn(["bun", "run", entry, ...args], {
    stdin: new Blob([stdin]),
    stdout: "pipe",
    stderr: "pipe",
    env,
  });
  const stdout = await new Response(proc.stdout).text();
  const code = await proc.exited;
  return { code, stdout };
}

/** Protocol run: every stdout line is a JSON response. */
async function run(
  stdin: string,
): Promise<{
  code: number;
  lines: (ParserResponse | ParserErrorResponse)[];
  stdout: string;
}> {
  const { code, stdout } = await spawnHelper(stdin);
  const lines = stdout
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l) as ParserResponse | ParserErrorResponse);
  return { code, lines, stdout };
}

describe("wth-app-parser protocol", () => {
  test("--version prints the library hash", async () => {
    const r = await spawnHelper("", "--version");
    expect(r.code).toBe(0);
    expect(r.stdout).toBe(`wth-app-parser ${libraryHash}\n`);
  });

  test("EOF with no requests exits 0 and prints nothing", async () => {
    const r = await run("");
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("");
  });

  test("serves several requests in one process, in order, one JSON line each", async () => {
    const reqs = [
      {
        id: "1",
        op: "check",
        source: 'root = Stack([TextContent("hi")])',
        tools: [],
      },
      {
        id: "2",
        op: "merge_check",
        base: 'root = Stack([t])\nt = TextContent("a")\nu = TextContent("dead")',
        source: 't = TextContent("b")',
        tools: [],
      },
      {
        id: "3",
        op: "check",
        source: 'root = Stack([q])\nq = Query("nope", {})',
        tools: [],
      },
    ];
    const r = await run(`${reqs.map((x) => JSON.stringify(x)).join("\n")}\n`);
    expect(r.code).toBe(0);
    expect(r.lines.map((l) => l.id)).toEqual(["1", "2", "3"]);
    const [a, b, c] = r.lines as ParserResponse[];
    expect(a!.diagnostics).toEqual([]);
    expect(a!.library_hash).toBe(libraryHash);
    expect(b!.source).toBe('root = Stack([t])\nt = TextContent("b")');
    expect(c!.diagnostics.map((d) => d.code)).toEqual(["tool-unknown"]);
    for (const line of r.lines) expect(Object.keys(line)).not.toContain("root");
  });

  test("malformed lines yield an error line with the id when recoverable and keep the stream alive", async () => {
    const r = await run(
      [
        "not json",
        JSON.stringify({ id: "x", op: "explode", source: "" }),
        JSON.stringify({
          id: "y",
          op: "merge_check",
          source: "a = 1",
          tools: [],
        }),
        JSON.stringify({
          id: "z",
          op: "check",
          source: "root = Stack([])",
          tools: [],
        }),
      ].join("\n"),
    );
    expect(r.code).toBe(0);
    expect(r.lines).toEqual([
      { id: null, error: expect.stringContaining("invalid JSON") },
      { id: "x", error: 'op must be "check" or "merge_check"' },
      { id: "y", error: "merge_check requires string field base" },
      expect.objectContaining({ id: "z", diagnostics: [] }),
    ]);
  });

  test("a request line above 4 MiB is refused without being parsed", async () => {
    const huge = JSON.stringify({
      id: "big",
      op: "check",
      source: "a".repeat(5 * 1024 * 1024),
      tools: [],
    });
    const r = await run(
      `${huge}\n${JSON.stringify({ id: "after", op: "check", source: "root = Stack([])", tools: [] })}\n`,
    );
    expect(r.code).toBe(0);
    expect(r.lines[0]).toEqual({
      id: null,
      error: expect.stringContaining("exceeds"),
    });
    expect(r.lines[1]).toEqual(expect.objectContaining({ id: "after" }));
  });
});
