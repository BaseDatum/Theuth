---
root: /Users/Sky.Moore/Develop/theuth-71-s9
---
# How the surface crate renders a fence

## What and why

A fence is a triple-backtick code block in markdown. The surface crate takes `json-render` and `mermaid` fences from agent replies and renders them as styled terminal output — lines of spans with foreground colors and dim/bold attributes. This lets a model draw diagrams and structured data that appear inline in the user's chat, as if they were prose.

The rendering is stateless: each fence is independent, parsed fresh, and turned into lines on demand. Width is the only input beyond the fence itself.

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
          {"id": "lib", "label": "lib.rs", "kind": "box", "ref": "crates/surface/src/lib.rs#L150-L164"},
          {"id": "fences", "label": "fences.rs", "kind": "rounded", "ref": "crates/surface/src/fences.rs#L24-L65"},
          {"id": "render", "label": "render.rs", "kind": "rounded", "ref": "crates/surface/src/render.rs#L42-L70"},
          {"id": "parse", "label": "parse", "kind": "rounded", "ref": "crates/surface/src/lib.rs#L90-L112"},
          {"id": "spec_lines", "label": "spec_lines", "kind": "rounded", "ref": "crates/surface/src/render.rs#L20-L22"},
          {"id": "mermaid", "label": "mermaid.rs", "kind": "rounded", "ref": "crates/surface/src/mermaid.rs#L16-L53"}
        ],
        "edges": [
          {"from": "lib", "to": "fences", "label": "finds"},
          {"from": "lib", "to": "parse", "label": "validates"},
          {"from": "render", "to": "parse", "label": "parses"},
          {"from": "render", "to": "spec_lines", "label": "renders json"},
          {"from": "render", "to": "mermaid", "label": "renders mmd"}
        ]
      }
    }
  }
}
```

The [check function](code:crates/surface/src/lib.rs#L150-L164) drives validation by iterating fences; [fence()](code:crates/surface/src/render.rs#L42-L70) renders them. Parsing lives in [lib.rs](code:crates/surface/src/lib.rs#L90-L112), rendering in [render.rs](code:crates/surface/src/render.rs#L20-L70) and [mermaid.rs](code:crates/surface/src/mermaid.rs#L16-L53).

## How it works

```json-render
{
  "root": "s",
  "elements": {
    "s": {
      "type": "Sequence",
      "props": {
        "actors": [
          {"id": "check", "label": "check"},
          {"id": "fences", "label": "fences()"},
          {"id": "parse", "label": "parse()"},
          {"id": "fence", "label": "fence()"},
          {"id": "render", "label": "spec_lines()"}
        ],
        "steps": [
          {"from": "check", "to": "fences", "label": "iterate text", "ref": "crates/surface/src/lib.rs#L152-L154"},
          {"from": "fences", "to": "check", "label": "Fence{lang, body, range}", "ref": "crates/surface/src/fences.rs#L45-L50", "style": "return"},
          {"from": "check", "to": "parse", "label": "parse(body)", "ref": "crates/surface/src/lib.rs#L159"},
          {"from": "parse", "to": "check", "label": "Ok/Err", "ref": "crates/surface/src/lib.rs#L90-L112", "style": "return"},
          {"from": "fence", "to": "fence", "label": "match lang", "ref": "crates/surface/src/render.rs#L44-L65"},
          {"from": "fence", "to": "parse", "label": "parse json-render", "ref": "crates/surface/src/render.rs#L54-L58"},
          {"from": "fence", "to": "render", "label": "spec_lines", "ref": "crates/surface/src/render.rs#L56"},
          {"from": "render", "to": "fence", "label": "Vec<Line>", "ref": "crates/surface/src/render.rs#L20-L22", "style": "return"}
        ]
      }
    }
  }
}
```

The `check` function validates every json-render fence in a text. It calls `fences(text)` to [iterate](code:crates/surface/src/lib.rs#L152-L154) over all fences, [filtering](code:crates/surface/src/lib.rs#L153) to only json-render ones, then tries to [parse each body](code:crates/surface/src/lib.rs#L159) and collects errors.

The `fence` function renders a single fence for display. It [matches on the language](code:crates/surface/src/render.rs#L44-L65): mermaid goes to `mermaid_lines`, json-render goes to `parse` then `spec_lines`. The function [returns None](code:crates/surface/src/render.rs#L65) for unrecognized languages and [logs](code:crates/surface/src/render.rs#L68) timing and success for every render.

The `fences` iterator [splits the text into lines](code:crates/surface/src/fences.rs#L27-L30) with byte offsets, then walks forward looking for [opening markers](code:crates/surface/src/fences.rs#L35-L38) (triple backticks or tildes). When it finds one, it [searches ahead](code:crates/surface/src/fences.rs#L40) for the closing line and [yields a Fence](code:crates/surface/src/fences.rs#L45-L50) with the body slice, byte range, and whether it closed.

```json-render
{
  "root": "e1",
  "elements": {
    "e1": {
      "type": "Excerpt",
      "props": {
        "path": "crates/surface/src/fences.rs",
        "text": "                    Fence {\n                        lang,\n                        body: &text[body_start..cstart],\n                        range: start..cstart + cline.trim_end_matches(['\\n', '\\r']).len(),\n                        closed: true,\n                    }",
        "start": 45,
        "lang": "rust",
        "caption": "Yielding a closed fence with its body slice and byte range"
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
        "path": "crates/surface/src/render.rs",
        "text": '    let (lines, ok, repaired) = match lang {\n        "mermaid" => match mermaid_lines(body, width) {\n            Ok(Some(lines)) => (Some(lines), true, false),\n            Ok(None) => (None, false, false),\n            Err(msg) => {\n                let first = msg.lines().next().unwrap_or_default();\n                tracing::debug!(target: "surface", kind = lang, "omitted: {first}");\n                (Some(omitted()), false, false)\n            }\n        },\n        "json-render" => match parse(body) {\n            Ok(p) => {\n                let lines = spec_lines(&p.spec, width);\n                (lines, true, p.repaired)\n            }\n            Err(errs) => {\n                let first = errs.first().map(ToString::to_string).unwrap_or_default();\n                tracing::debug!(target: "surface", kind = lang, "omitted: {first}");\n                (Some(omitted()), false, false)\n            }\n        },',
        "start": 44,
        "lang": "rust",
        "caption": "Dispatching to the right renderer by language, with fallback to omitted()"
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
        "path": "crates/surface/src/lib.rs",
        "text": '    for (i, f) in fences::fences(text)\n        .filter(|f| f.lang == "json-render")\n        .enumerate()\n    {\n        let ordinal = i + 1;\n        if !f.closed {\n            out.push((ordinal, vec![err("", "fence not closed")]));\n        } else if let Err(errs) = parse(f.body) {\n            out.push((ordinal, errs));\n        }\n    }',
        "start": 152,
        "lang": "rust",
        "caption": "Validating json-render fences: iterate, filter, parse, collect errors"
      }
    }
  }
}
```

## Decisions and trade-offs

**Stateless rendering.** Each fence is parsed fresh on every render. There is no cached AST or incremental update. This keeps the implementation simple and the memory footprint small: a fence's lifetime is one function call.

**Width-driven layout.** The only context is [width](code:crates/surface/src/render.rs#L42): narrow terminals get simplified output (adjacency lists instead of graph diagrams), wide ones get full box-drawing. The [width gates the render](code:crates/surface/src/render.rs#L980-L994) test confirms that graphs fall back below a threshold.

**Error recovery.** When a fence fails to parse or render, the system [returns a placeholder](code:crates/surface/src/render.rs#L51) ("content omitted") and [logs the error](code:crates/surface/src/render.rs#L50) rather than breaking the whole message. The user sees something; the model gets correction feedback via surface_error.

## Checks

* [valid_fixtures_match_golden_files](code:crates/surface/src/render.rs#L939-L977) renders every fixture and diffs against committed golden output
* [fence_hook_omits_failures_quietly](code:crates/surface/src/render.rs#L997-L1008) confirms that broken fences fall back gracefully
* [check_reports_only_failing_fences_by_ordinal](code:crates/surface/src/lib.rs#L376-L386) validates that check() finds errors in the right fences
* [ranges_cover_fence_lines_and_bodies_exclude_them](code:crates/surface/src/fences.rs#L101-L115) proves the iterator's byte ranges are correct
