import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { isReactiveSchema } from "@openuidev/lang-core";
import { openuiLibrary } from "@openuidev/react-ui/genui-lib";
import { check, generatedLibrary } from "../src/check.ts";
import {
  CURATED_COMPONENTS,
  LIBRARY_ROOT,
  buildCuratedLibrary,
  library,
  promptExamples,
  promptOptions,
  reactiveProps,
} from "../src/library.ts";
import { DIAGRAMS } from "../src/diagrams.ts";

const SHARED_DEFS = ["ActionExpression"];
const diagramNames = ["Graph", ...Object.keys(DIAGRAMS)];
const sig = (name: string) => library.toSpec().components[name]!.signature;
const members = (name: string) =>
  new Set(
    sig(name)
      .match(/[A-Z][A-Za-z]+/g)!
      .filter((n) => n in library.components),
  );

describe("curated library", () => {
  test("Card is the root and Stack stays a component", () => {
    expect(LIBRARY_ROOT).toBe("Card");
    expect(library.root).toBe("Card");
    expect(generatedLibrary.root).toBe("Card");
    expect(CURATED_COMPONENTS).toContain("Stack");
    expect(sig("Card")).toContain("| Stack");
    expect(sig("Stack")).toStartWith("Stack(children: any[]");
  });

  test("library order is the curated order (it is hashed)", () => {
    expect(Object.keys(library.components)).toEqual([...CURATED_COMPONENTS]);
  });

  test("schema is dependency-closed: every $ref names a curated component and nothing else is defined", () => {
    const schema = library.toJSONSchema();
    const defs = Object.keys(schema.$defs ?? {})
      .filter((d) => !SHARED_DEFS.includes(d))
      .sort();
    expect(defs).toEqual([...CURATED_COMPONENTS].sort());
    const refs = JSON.stringify(schema)
      .match(/"#\/\$defs\/([A-Za-z]+)"/g)!
      .map((m) => m.slice(9, -1));
    const curated: readonly string[] = [...CURATED_COMPONENTS, ...SHARED_DEFS];
    for (const r of refs) expect(curated).toContain(r);
  });

  test("the pruned upstream components are gone from every union", () => {
    const text = JSON.stringify(library.toJSONSchema());
    for (const gone of ["RadialChart", "Slice", "VisualCardBlock", "VisualCardItem", "MetricIndicatorWithStrikethrough"])
      expect(text).not.toContain(`"${gone}"`);
    for (const kept of ["SectionBlock", "FollowUpBlock", "Form", "FormControl", "DatePicker", "Modal", "EditableTable", "IconButton", "InlineHeader", "EntityList", "ImageBlock", "CompositeCardBlock"])
      expect(CURATED_COMPONENTS).toContain(kept as never);
  });

  test("container unions take the added upstream blocks and every local diagram, keeping upstream renderers", () => {
    for (const parent of ["Card", "TabItem", "AccordionItem", "Carousel", "Modal", "SectionItem", "CompositeCardItem"]) {
      const m = members(parent);
      for (const d of diagramNames) expect(m.has(d)).toBe(true);
    }
    const card = members("Card");
    for (const n of ["TextContent", "Tabs", "Stack", "Form", "EditableTable", "SectionBlock", "FollowUpBlock", "ListBlock", "Modal"])
      expect(card.has(n)).toBe(true);
    // Nested containers do not offer themselves or the page-level blocks.
    expect(members("TabItem").has("SectionBlock")).toBe(false);
    expect(members("SectionItem").has("SectionBlock")).toBe(false);
    expect(members("SectionItem").has("Tabs")).toBe(true);
    expect(library.components.Card!.component).toBe(openuiLibrary.components.Card!.component);
    expect(library.components.Card!.description).toBe(openuiLibrary.components.Card!.description);
    expect(library.components.TabItem!.component).toBe(openuiLibrary.components.TabItem!.component);
  });

  test("components without local references keep upstream schema identity (reactive markers survive)", () => {
    expect(library.components.Input!.props).toBe(openuiLibrary.components.Input!.props);
    expect(library.components.Form!.props).toBe(openuiLibrary.components.Form!.props);
    expect(isReactiveSchema(library.components.Select!.props._zod.def.shape.value)).toBeTrue();
    const reactive = reactiveProps(library);
    expect(reactive).toMatchObject({
      Callout: ["visible"],
      Input: ["value"],
      Select: ["value"],
      Modal: ["open"],
      Graph: ["highlight"],
      Fishbone: ["highlight"],
    });
    for (const d of diagramNames) expect(reactive[d]).toEqual(["highlight"]);
    expect(reactive).toEqual(generatedLibrary.reactive);
  });

  test("a subset that strands a required child type is a generation error, not a silent any", () => {
    expect(() => buildCuratedLibrary(["Card", "Buttons"], "Card")).toThrow(
      /\[extend\] Cannot remove "Button"/,
    );
    expect(() => buildCuratedLibrary(["Card", "Table"], "Card")).toThrow(/\[extend\] Cannot remove "Col"/);
    expect(() => buildCuratedLibrary(["Card", "Nope"], "Card")).toThrow(/neither an upstream/);
    expect(() => buildCuratedLibrary(["Stack"], "Card")).toThrow(/root "Card"/);
  });

  test("generated JSON matches the live library definition and preserves positional key order", () => {
    const schema = library.toJSONSchema();
    expect(JSON.stringify(generatedLibrary.schema)).toBe(JSON.stringify(schema));
    expect(Object.keys(generatedLibrary.schema.$defs!.Button!.properties!)).toEqual([
      "label",
      "action",
      "variant",
      "type",
      "size",
    ]);
    expect(generatedLibrary.components).toEqual([...CURATED_COMPONENTS]);
    expect(generatedLibrary.hash).toMatch(/^[0-9a-f]{64}$/);
  });
});

describe("pure functions and the typed action", () => {
  const fn = (name: string, args: Record<string, unknown>) =>
    (library.functions![name]!.fn as (a: unknown) => unknown)(args);

  test("Percent, FormatNumber and FormatDate are deterministic en-US/UTC strings", () => {
    expect(fn("Percent", { part: 3, total: 4 })).toBe("75%");
    expect(fn("Percent", { part: 1, total: 3, digits: 1 })).toBe("33.3%");
    expect(fn("Percent", { part: 1, total: 0 })).toBe("0%");
    expect(fn("FormatNumber", { value: 1234.5 })).toBe("1,234.5");
    expect(fn("FormatNumber", { value: 1234, notation: "compact" })).toBe("1.2K");
    expect(fn("FormatNumber", { value: 1234.5, digits: 2, currency: "usd" })).toBe("$1,234.50");
    expect(fn("FormatDate", { value: "2026-01-05" })).toBe("Jan 5, 2026");
    expect(fn("FormatDate", { value: "2026-01-05", style: "long" })).toBe("January 5, 2026");
    expect(fn("FormatDate", { value: Date.UTC(2026, 0, 5, 14, 30), style: "iso" })).toBe("2026-01-05");
    expect(fn("FormatDate", { value: "2026-01-05T14:30:00Z", style: "datetime" })).toBe("Jan 5, 2026, 14:30 UTC");
    expect(() => fn("FormatDate", { value: "not a date" })).toThrow();
  });

  test("they and CopyToClipboard are in the generated schema the in-process check reads", () => {
    const { functions, actions } = generatedLibrary.schema;
    expect(Object.keys(functions ?? {})).toEqual(expect.arrayContaining(["Percent", "FormatNumber", "FormatDate"]));
    expect(Object.keys(actions ?? {})).toEqual(["CopyToClipboard"]);
    expect(Object.keys(actions!.CopyToClipboard!.params.properties!)).toEqual(["text"]);
    expect(Object.keys(library.actions ?? {})).toEqual(["CopyToClipboard"]);
  });
});

describe("prompt guidance", () => {
  test("upstream rules are filtered, not dropped: the app rules and the chat rules that still apply survive", () => {
    const rules = promptOptions.additionalRules;
    expect(rules[0]).toContain("root = Card([...])");
    expect(rules.some((r) => r.startsWith("Use FollowUpBlock at the END"))).toBe(true);
    expect(rules.some((r) => r.startsWith("Every Form MUST have EXACTLY ONE submit Button"))).toBe(true);
    // Chat-only wiring (Card sources, Stack ban) does not leak in.
    expect(rules.join("\n")).not.toMatch(/Do NOT use Stack|sources on Card|VisualCard|WithStrikethrough/);
  });

  test("every example passes the strict check against the generated schema, with a Card root", () => {
    expect(promptExamples.length).toBeGreaterThanOrEqual(5);
    for (const ex of promptExamples) {
      const source = ex.slice(ex.indexOf("\n\n") + 2);
      expect(source).toStartWith("root = Card([");
      const result = check(source, []);
      expect({ ex: ex.split("\n")[0], diagnostics: result.diagnostics }).toEqual({
        ex: ex.split("\n")[0],
        diagnostics: [],
      });
    }
  });

  test("generated catalog carries signatures, functions, the custom action, rules and examples", () => {
    const text = readFileSync(join(import.meta.dir, "../generated/catalog.txt"), "utf8");
    expect(text).toContain(`root statement: root = Card([...])`);
    expect(text).toContain("## Component Signatures");
    expect(text).toMatch(/^SectionBlock\(/m);
    expect(text).toMatch(/^@FormatNumber\(/m);
    expect(text).toMatch(/^- @CopyToClipboard\(text: string\)/m);
    expect(text).toContain("Pass one @-prefixed step bare");
    expect(text).toContain("## Rules");
    expect(text).toContain("## Examples");
    for (const ex of promptExamples) expect(text).toContain(ex);
  });
});
