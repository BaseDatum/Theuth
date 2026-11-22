/**
 * Review m6: `check.ts` reads the positional index of `onSelect` from the generated schema (so the in-process
 * bundle carries neither diagrams.ts nor zod). It must equal what the diagram definitions themselves yield:
 * the wire props in catalog order, then `highlight`, then `onSelect`; `Graph` keeps 65's order.
 */
import { expect, test } from "bun:test";
import { SELECT_AT } from "../src/check.ts";
import { DIAGRAMS } from "../src/diagrams.ts";

test("SELECT_AT from the generated schema equals the DIAGRAMS-derived table", () => {
  const fromDiagrams: Record<string, number> = { Graph: 5 };
  for (const [name, d] of Object.entries(DIAGRAMS)) fromDiagrams[name] = Object.keys(d.props.shape).length + 1;
  expect(SELECT_AT).toEqual(fromDiagrams);
  expect(Object.keys(SELECT_AT).length).toBe(Object.keys(DIAGRAMS).length + 1);
});
