---
root: /Users/Sky.Moore/Develop/theuth-71-s9
---
# What commit bc816f2a changed

## What and why

This commit adds board infrastructure to the surface crate: interactive anchors that link prose and diagrams to code. The reader hovers a sentence and the corresponding code lines light up; clicks to open the file beside the text; tours a diagram stop by stop.

The commit introduces three pieces: [Excerpt](code:crates/surface/src/spec.rs#L710-L721) blocks that quote code with numbered gutters and marked rows, [Sequence](code:crates/surface/src/spec.rs#L580-L591) diagrams for interactions over time, and `ref` fields on [GraphNode](code:crates/surface/src/spec.rs#L478-L490) that let every diagram unit point at code. All three use a new [anchor grammar](code:crates/surface/src/anchor.rs#L16-L29) that parses `path#L10-L24` and validates the anchors against the repository root.

## Map

```json-render
{
  "root": "g",
  "elements": {
    "g": {
      "type": "Graph",
      "props": {
        "direction": "TB",
        "nodes": [
          {"id": "spec", "label": "spec.rs", "ref": "crates/surface/src/spec.rs#L1", "kind": "rounded"},
          {"id": "anchor", "label": "anchor.rs", "ref": "crates/surface/src/anchor.rs#L1", "kind": "rounded", "state": "active"},
          {"id": "sequence", "label": "sequence.rs", "ref": "crates/surface/src/sequence.rs#L1", "kind": "rounded", "state": "active"},
          {"id": "validate", "label": "validate.rs", "ref": "crates/surface/src/validate.rs#L1", "kind": "rounded"},
          {"id": "render", "label": "render.rs", "ref": "crates/surface/src/render.rs#L1", "kind": "rounded"},
          {"id": "catalog", "label": "catalog.rs", "ref": "crates/surface/src/catalog.rs#L1", "kind": "rounded"},
          {"id": "fixtures", "label": "fixtures/", "ref": "crates/surface/fixtures/json-render/valid-sequence.json#L1", "kind": "cylinder"}
        ],
        "edges": [
          {"from": "spec", "to": "validate", "label": "defines"},
          {"from": "spec", "to": "render", "label": "defines"},
          {"from": "spec", "to": "catalog", "label": "counts"},
          {"from": "validate", "to": "anchor", "label": "checks"},
          {"from": "sequence", "to": "spec", "label": "lowers"},
          {"from": "render", "to": "spec", "label": "formats"},
          {"from": "fixtures", "to": "validate", "label": "tests"}
        ]
      }
    }
  }
}
```

Two new files ([anchor.rs](code:crates/surface/src/anchor.rs#L1), [sequence.rs](code:crates/surface/src/sequence.rs#L1)) and changes across five existing ones. The anchor parser validates references; sequence lowers interaction diagrams to mermaid; spec defines the new types; validate checks them; render formats them for display.

## How it works

```json-render
{
  "root": "s",
  "elements": {
    "s": {
      "type": "Sequence",
      "props": {
        "title": "Validating and rendering an anchored component",
        "actors": [
          {"id": "spec", "label": "spec.rs"},
          {"id": "validate", "label": "validate.rs"},
          {"id": "anchor", "label": "anchor.rs"},
          {"id": "render", "label": "render.rs"},
          {"id": "sequence", "label": "sequence.rs"}
        ],
        "steps": [
          {"from": "spec", "to": "validate", "label": "validate(Component)", "ref": "crates/surface/src/validate.rs#L32"},
          {"from": "validate", "to": "validate", "label": "match component", "ref": "crates/surface/src/validate.rs#L65"},
          {"from": "validate", "to": "validate", "label": "sequence()", "ref": "crates/surface/src/validate.rs#L375"},
          {"from": "validate", "to": "anchor", "label": "parse(ref)", "ref": "crates/surface/src/anchor.rs#L32"},
          {"from": "anchor", "to": "anchor", "label": "check_path()", "ref": "crates/surface/src/anchor.rs#L78"},
          {"from": "anchor", "to": "validate", "label": "Ok(Anchor)", "ref": "crates/surface/src/anchor.rs#L105"},
          {"from": "validate", "to": "spec", "label": "errors or valid", "ref": "crates/surface/src/validate.rs#L72"},
          {"from": "spec", "to": "render", "label": "render(Component)", "ref": "crates/surface/src/render.rs#L340"},
          {"from": "render", "to": "sequence", "label": "to_mermaid()", "ref": "crates/surface/src/sequence.rs#L28"},
          {"from": "sequence", "to": "render", "label": "sequenceDiagram", "ref": "crates/surface/src/sequence.rs#L66"}
        ]
      }
    }
  }
}
```

When a user sends a json-render block with a [Sequence](code:crates/surface/src/spec.rs#L580-L591) or [Excerpt](code:crates/surface/src/spec.rs#L710-L721), [validate()](code:crates/surface/src/validate.rs#L32) dispatches to component-specific checks. For every `ref` field, [anchor_ref()](code:crates/surface/src/validate.rs#L299-L310) calls the [anchor parser](code:crates/surface/src/anchor.rs#L32) which checks the grammar and validates the path against the repository root via [check_path()](code:crates/surface/src/anchor.rs#L78-L106).

If validation succeeds, [render()](code:crates/surface/src/render.rs#L340) formats the component. For Sequence, it calls [to_mermaid()](code:crates/surface/src/sequence.rs#L28-L60) which lowers the actors and steps to a sequenceDiagram string. For Excerpt, [element()](code:crates/surface/src/render.rs#L372-L389) formats the code with a numbered gutter and marks the specified rows.

```json-render
{
  "root": "e1",
  "elements": {
    "e1": {
      "type": "Excerpt",
      "props": {
        "path": "crates/surface/src/anchor.rs",
        "text": "#[derive(Clone, Debug, PartialEq, Eq)]\npub enum Anchor {\n    /// A file, or a 1-based inclusive line range inside it (`#L<a>` is `(a, a)`).\n    Code {\n        /// Root-relative path.\n        path: String,\n        /// `(first, last)` line, inclusive; `None` for the whole file.\n        lines: Option<(u32, u32)>,\n    },\n    /// A block of the same board.\n    Block {\n        /// The block id.\n        id: String,\n    },",
        "start": 16,
        "lang": "rust",
        "caption": "The anchor grammar: code references with optional line ranges, or block ids",
        "symbol": "Anchor"
      }
    }
  }
}
```

```json-render
{
  "root": "e2",
  "elements": {
    "e2": {
      "type": "Excerpt",
      "props": {
        "path": "crates/surface/src/validate.rs",
        "text": "fn anchor_ref(path: String, r: Option<&str>, errs: &mut Vec<SpecError>) {\n    if let Some(r) = r\n        && let Err(msg) = anchor::parse(r)\n    {\n        errs.push(err(&path, msg));\n    }\n}\n\n/// `Graph` checks serde cannot express (65 \u00a73.2): unique node and group ids, edge endpoints and\n/// node groups that exist, the caps, the label length and the `ref` grammar. Self-loops are fine.\nfn graph(g: &GraphProps, p: &str, errs: &mut Vec<SpecError>) {\n    if g.nodes.len() > MAX_GRAPH_NODES {",
        "start": 299,
        "lang": "rust",
        "caption": "Every ref field is validated through the anchor parser",
        "symbol": "anchor_ref"
      }
    }
  }
}
```

```json-render
{
  "root": "e3",
  "elements": {
    "e3": {
      "type": "Excerpt",
      "props": {
        "path": "crates/surface/src/spec.rs",
        "text": "pub struct ExcerptProps {\n    /// Root-relative file path; no `..`, no scheme.\n    pub path: String,\n    /// The quoted lines, verbatim.\n    pub text: String,\n    /// 1-based line number of the first quoted line; default 1.\n    #[schemars(range(min = 1))]\n    pub start: Option<u32>,\n    /// Language name, e.g. `rust`.\n    pub lang: Option<String>,\n    /// One line under the code: the claim these lines carry.\n    pub caption: Option<String>,\n    /// Marked rows as `[from, to]` line-number pairs, inclusive, inside the quoted range.\n    pub mark: Option<Vec<[u32; 2]>>,\n    /// Name of the definition the range quotes; enables `moved` detection.",
        "start": 710,
        "lang": "rust",
        "caption": "Excerpt props: path, text, optional line numbers, marks, and symbol tracking",
        "symbol": "ExcerptProps",
        "mark": [[713, 714], [720, 721]]
      }
    }
  }
}
```

## Decisions and trade-offs

The anchor grammar is deliberately minimal: root-relative paths only (no `..`, no absolute paths, no URL schemes) to keep boards portable and prevent references outside the repository. Line ranges are inclusive and 1-based to match editor conventions.

Limits keep blocks renderable across channels:

```json-render
{
  "root": "k",
  "elements": {
    "k": {
      "type": "KeyValueList",
      "props": {
        "items": [
          {"key": "Sequence actors", "value": "2–12 (readability)"},
          {"key": "Sequence steps", "value": "1–40 (scroll fatigue)"},
          {"key": "Excerpt size", "value": "120 lines / 16 KiB (prevents abuse)"}
        ]
      }
    }
  }
}
```

## Checks

The commit adds validation tests for the new components ([anchor grammar](code:crates/surface/src/anchor.rs#L111-L145), [sequence validation](code:crates/surface/src/validate.rs#L835-L889), [excerpt checks](code:crates/surface/src/validate.rs#L424-L496)) and rendering fixtures ([valid-excerpt.json](code:crates/surface/fixtures/json-render/valid-excerpt.json#L1), [valid-sequence.json](code:crates/surface/fixtures/json-render/valid-sequence.json#L1)). Golden files verify the text and mermaid outputs at two column widths.

```json-render
{
  "root": "c",
  "elements": {
    "c": {
      "type": "Checklist",
      "props": {
        "items": [
          {"text": "Anchor grammar accepts code forms and blocks", "done": true},
          {"text": "Anchor grammar rejects schemes and bad ranges", "done": true},
          {"text": "Sequence validation checks actors and refs", "done": true},
          {"text": "Excerpt validation checks marks and ranges", "done": true},
          {"text": "Golden files verify text rendering at w60/w100", "done": true},
          {"text": "Sequence lowers to valid mermaid diagrams", "done": true}
        ]
      }
    }
  }
}
```
