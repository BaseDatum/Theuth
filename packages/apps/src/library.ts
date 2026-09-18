/**
 * Curated, dependency-closed subset of the upstream `openuiLibrary` (design 46 §4).
 *
 * Every component keeps its upstream renderer, description and field schemas. Only fields that
 * reference other components (child unions such as `Card.children`) are rebuilt so they mention
 * exclusively components in this subset; everything else keeps object identity, so upstream
 * `reactive()` markers, `tagSchemaId` tags and `.describe()` metadata survive untouched.
 *
 * This module pulls the whole react-ui graph and is for the frontend and the generation script.
 * The headless helper never imports it; it reads `generated/library.json`.
 */
import { createLibrary } from "@openuidev/react-lang";
import type {
  ComponentGroup,
  DefinedComponent,
  Library,
} from "@openuidev/react-lang";
import { defineComponent, isReactiveSchema } from "@openuidev/lang-core";
import {
  openuiComponentGroups,
  openuiLibrary,
} from "@openuidev/react-ui/genui-lib";
import { z } from "zod/v4";
import type * as core from "zod/v4/core";

/** Ordered component names; order is part of the compatibility hash. */
export const CURATED_COMPONENTS = [
  // layout
  "Stack",
  "Card",
  "CardHeader",
  "Separator",
  // content
  "TextContent",
  "Image",
  // data
  "Table",
  "Col",
  "BarChart",
  "LineChart",
  "AreaChart",
  "PieChart",
  "Series",
  // input
  "Input",
  "TextArea",
  "Select",
  "SelectItem",
  "SwitchGroup",
  "SwitchItem",
  // actions
  "Button",
  "Buttons",
] as const;

export type CuratedComponentName = (typeof CURATED_COMPONENTS)[number];

export const LIBRARY_ROOT: CuratedComponentName = "Stack";

type AnyZod = core.$ZodType;
type UpstreamComponent = (typeof openuiLibrary.components)[string];

interface Def {
  type: string;
  innerType?: AnyZod;
  element?: AnyZod;
  options?: AnyZod[];
  shape?: Record<string, AnyZod>;
  defaultValue?: unknown;
}

const def = (s: AnyZod): Def => s._zod.def as unknown as Def;

/** Copy `.describe()`/`.meta()` metadata from an upstream schema onto a rebuilt one. */
function withMeta<T extends AnyZod>(from: AnyZod, to: T): T {
  const meta = z.globalRegistry.get(from);
  return meta ? ((to as unknown as z.ZodType).meta(meta) as unknown as T) : to;
}

/**
 * Build the curated library. `keep` must be dependency-closed: a required field whose component
 * union would become empty is a generation error, not a silent `any`.
 */
export function buildCuratedLibrary(
  keep: readonly string[] = CURATED_COMPONENTS,
  root: string = LIBRARY_ROOT,
): Library {
  const upstream = openuiLibrary.components;
  const propsToName = new Map<AnyZod, string>();
  for (const [name, comp] of Object.entries(upstream))
    propsToName.set(comp.props as AnyZod, name);
  const keepSet = new Set(keep);
  for (const name of keep)
    if (!upstream[name])
      throw new Error(
        `[apps/library] "${name}" is not an upstream openuiLibrary component`,
      );

  const pruned = new Map<string, AnyZod>();
  const visiting = new Set<string>();

  /** Rebuild `schema` without references to excluded components. `undefined` = the schema itself is an excluded component ref. */
  function prune(
    schema: AnyZod,
    owner: string,
    path: string,
  ): AnyZod | undefined {
    const refName = path === "" ? undefined : propsToName.get(schema);
    if (refName !== undefined) {
      if (!keepSet.has(refName)) return undefined;
      return prunedProps(refName);
    }
    const d = def(schema);
    switch (d.type) {
      case "optional":
      case "nullable":
      case "default": {
        const inner = prune(d.innerType!, owner, path);
        if (!inner) return undefined;
        if (inner === d.innerType) return schema;
        const z4 = inner as unknown as z.ZodType;
        const wrapped =
          d.type === "optional"
            ? z4.optional()
            : d.type === "nullable"
              ? z4.nullable()
              : z4.default(d.defaultValue as never);
        return withMeta(schema, wrapped as unknown as AnyZod);
      }
      case "array": {
        const el = prune(d.element!, owner, path);
        if (!el) return undefined;
        if (el === d.element) return schema;
        return withMeta(
          schema,
          z.array(el as unknown as z.ZodType) as unknown as AnyZod,
        );
      }
      case "union": {
        const options = d.options!;
        const kept = options
          .map((o) => prune(o, owner, path))
          .filter((o): o is AnyZod => o !== undefined);
        if (kept.length === 0) return undefined;
        if (
          kept.length === options.length &&
          kept.every((o, i) => o === options[i])
        )
          return schema;
        if (kept.length === 1) return kept[0];
        return withMeta(
          schema,
          z.union(
            kept as unknown as [z.ZodType, z.ZodType, ...z.ZodType[]],
          ) as unknown as AnyZod,
        );
      }
      case "object": {
        const shape = d.shape!;
        let changed = false;
        const next: Record<string, AnyZod> = {};
        for (const [key, field] of Object.entries(shape)) {
          const out = prune(field, owner, `${path}.${key}`);
          if (!out) {
            const required = !["optional", "default", "nullable"].includes(
              def(field).type,
            );
            if (required)
              throw new Error(
                `[apps/library] ${owner}${path}.${key} requires components outside the curated set; add them to CURATED_COMPONENTS`,
              );
            changed = true;
            continue;
          }
          if (out !== field) changed = true;
          next[key] = out;
        }
        if (!changed) return schema;
        return withMeta(
          schema,
          z.object(next as Record<string, z.ZodType>) as unknown as AnyZod,
        );
      }
      default:
        return schema;
    }
  }

  function prunedProps(name: string): AnyZod {
    const done = pruned.get(name);
    if (done) return done;
    if (visiting.has(name))
      throw new Error(
        `[apps/library] cyclic component schema reference through "${name}" is not supported by pruning`,
      );
    visiting.add(name);
    const comp = upstream[name]!;
    const out = prune(comp.props as AnyZod, name, "") ?? comp.props;
    visiting.delete(name);
    pruned.set(name, out as AnyZod);
    return out as AnyZod;
  }

  const components: DefinedComponent[] = keep.map((name) => {
    const comp = upstream[name]! as UpstreamComponent;
    const props = prunedProps(name);
    if (props === comp.props) return comp;
    // prune() only ever rebuilds an object root as z.object(), so the widened AnyZod is a $ZodObject here.
    const objectProps = props as core.$ZodObject;
    return defineComponent({
      name,
      props: objectProps,
      description: comp.description,
      component: comp.component,
    });
  });

  return createLibrary({
    components,
    componentGroups: curatedGroups(keepSet, Object.keys(upstream)),
    root,
    id: "wth-apps",
  });
}

/** Upstream notes that describe excluded components without naming them. */
const MISLEADING_NOTE_PREFIXES = ["- data is an array of { id, values } rows"];

/** Upstream groups restricted to kept components; notes that mention excluded components are dropped. */
function curatedGroups(
  keep: Set<string>,
  allNames: string[],
): ComponentGroup[] {
  const excluded = allNames.filter((n) => !keep.has(n));
  const mentionsExcluded = (note: string) =>
    excluded.some((n) => new RegExp(`\\b${n}\\b`).test(note)) ||
    MISLEADING_NOTE_PREFIXES.some((p) => note.startsWith(p));
  const groups: ComponentGroup[] = [];
  for (const g of openuiComponentGroups) {
    const components = g.components.filter((c) => keep.has(c));
    if (!components.length) continue;
    const notes = g.notes?.filter((n) => !mentionsExcluded(n));
    groups.push({
      name: g.name,
      components,
      ...(notes?.length ? { notes } : {}),
    });
  }
  return groups;
}

/** Prop names whose schema carries upstream's WeakSet `reactive()` marker (`$binding<…>`), per component. */
export function reactiveProps(library: Library): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const [name, comp] of Object.entries(library.components)) {
    const shape = def(comp.props as AnyZod).shape ?? {};
    const names = Object.entries(shape)
      .filter(([, s]) => isReactiveSchema(s))
      .map(([k]) => k);
    if (names.length) out[name] = names;
  }
  return out;
}

/** The curated library used by the frontend renderer. Built once per module load. */
export const library: Library = buildCuratedLibrary();
