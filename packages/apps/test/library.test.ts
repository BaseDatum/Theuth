import { describe, expect, test } from "bun:test";
import { isReactiveSchema } from "@openuidev/lang-core";
import { openuiLibrary } from "@openuidev/react-ui/genui-lib";
import { generatedLibrary } from "../src/check.ts";
import {
  CURATED_COMPONENTS,
  buildCuratedLibrary,
  library,
  reactiveProps,
} from "../src/library.ts";

describe("curated library", () => {
  test("schema is dependency-closed: every $ref names a curated component and nothing else is defined", () => {
    const schema = library.toJSONSchema();
    const defs = Object.keys(schema.$defs ?? {}).sort();
    expect(defs).toEqual([...CURATED_COMPONENTS].sort());
    const refs = JSON.stringify(schema)
      .match(/"#\/\$defs\/([A-Za-z]+)"/g)!
      .map((m) => m.slice(9, -1));
    const curated: readonly string[] = CURATED_COMPONENTS;
    for (const r of refs) expect(curated).toContain(r);
  });

  test("container child unions mention only curated components while keeping upstream renderers", () => {
    const sig = library.toSpec().components.Card!.signature;
    expect(sig).not.toContain("Tabs");
    expect(sig).not.toContain("MarkDownRenderer");
    expect(sig).toContain("TextContent");
    expect(library.components.Card!.component).toBe(
      openuiLibrary.components.Card!.component,
    );
    expect(library.components.Card!.description).toBe(
      openuiLibrary.components.Card!.description,
    );
  });

  test("components without excluded references keep upstream schema identity (reactive markers survive)", () => {
    expect(library.components.Input!.props).toBe(
      openuiLibrary.components.Input!.props,
    );
    expect(
      isReactiveSchema(library.components.Select!.props._zod.def.shape.value),
    ).toBeTrue();
    expect(reactiveProps(library)).toEqual({
      Input: ["value"],
      TextArea: ["value"],
      Select: ["value"],
      SwitchGroup: ["value"],
    });
  });

  test("a subset that drops a required child type is a generation error, not a silent any", () => {
    expect(() => buildCuratedLibrary(["Stack", "Buttons"], "Stack")).toThrow(
      /Buttons\.buttons requires components outside/,
    );
    expect(() => buildCuratedLibrary(["Stack", "Table"], "Stack")).toThrow(
      /Table\.columns/,
    );
  });

  test("generated JSON matches the live library definition and preserves positional key order", () => {
    const schema = library.toJSONSchema();
    expect(JSON.stringify(generatedLibrary.schema)).toBe(
      JSON.stringify(schema),
    );
    expect(
      Object.keys(generatedLibrary.schema.$defs!.Button!.properties!),
    ).toEqual(["label", "action", "variant", "type", "size"]);
    expect(generatedLibrary.components).toEqual([...CURATED_COMPONENTS]);
    expect(generatedLibrary.hash).toMatch(/^[0-9a-f]{64}$/);
  });
});
