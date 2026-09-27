---
root: /Users/Sky.Moore/Develop/theuth-71-s2
---
# How the surface crate parses a fence

## What and why

The surface crate identifies and extracts fenced code blocks from markdown text—the regions delimited by triple backticks that contain code, diagrams, or structured data. This parsing supports the system's ability to render json-render blocks, mermaid diagrams, and openui fences inline in chat.

A fence parser must handle asymmetric delimiters (opening has metadata, closing does not), preserve the body verbatim, and distinguish nested backticks in the content from actual fence boundaries.

## Map

```json-render
{
  "root": "r",
  "elements": {
    "r": {
      "type": "Graph",
      "props": {
        "direction": "TB",
        "nodes": [
          {"id": "lib", "label": "lib.rs", "kind": "box", "ref": "crates/surface/src/lib.rs#L150-L164"},
          {"id": "fences", "label": "fences()", "kind": "rounded", "ref": "crates/surface/src/fences.rs#L24-L65"},
          {"id": "fence", "label": "Fence", "kind": "box", "ref": "crates/surface/src/fences.rs#L11-L21"},
          {"id": "opening", "label": "opening()", "kind": "rounded", "ref": "crates/surface/src/fences.rs#L74-L86"},
          {"id": "closing", "label": "closing()", "kind": "rounded", "ref": "crates/surface/src/fences.rs#L89-L94"},
          {"id": "unindented", "label": "unindented()", "kind": "rounded", "ref": "crates/surface/src/fences.rs#L68-L71"}
        ],
        "edges": [
          {"from": "lib", "to": "fences", "label": "calls"},
          {"from": "fences", "to": "fence", "label": "yields"},
          {"from": "fences", "to": "opening", "label": "uses"},
          {"from": "fences", "to": "closing", "label": "uses"},
          {"from": "opening", "to": "unindented", "label": "calls"},
          {"from": "closing", "to": "unindented", "label": "calls"}
        ]
      }
    }
  }
}
```

The fence parser lives in fences.rs with four functions: fences() is the iterator factory, opening() and closing() recognize the boundary lines, and unindented() strips up to three leading spaces per CommonMark. Public functions like check() and fences_of() consume the iterator.

## How it works

The parser scans text line by line, looking for opening fence markers, then searching forward for the matching closing marker. When found, it yields a `Fence` struct with the language tag, body content, and byte range.

```json-render
{
  "root": "r",
  "elements": {
    "r": {
      "type": "Sequence",
      "props": {
        "actors": [
          {"id": "caller", "label": "caller"},
          {"id": "fences", "label": "fences()"},
          {"id": "opening", "label": "opening()"},
          {"id": "closing", "label": "closing()"}
        ],
        "steps": [
          {"from": "caller", "to": "fences", "label": "text", "ref": "crates/surface/src/lib.rs#L152-L154"},
          {"from": "fences", "to": "fences", "label": "split into lines", "ref": "crates/surface/src/fences.rs#L25-L29"},
          {"from": "fences", "to": "opening", "label": "each line", "ref": "crates/surface/src/fences.rs#L34"},
          {"from": "opening", "to": "fences", "label": "(marker, len, lang)?", "style": "return", "ref": "crates/surface/src/fences.rs#L75-L86"},
          {"from": "fences", "to": "closing", "label": "scan forward", "ref": "crates/surface/src/fences.rs#L38"},
          {"from": "closing", "to": "fences", "label": "matches?", "style": "return", "ref": "crates/surface/src/fences.rs#L89-L94"},
          {"from": "fences", "to": "caller", "label": "Fence{lang, body, range}", "style": "return", "ref": "crates/surface/src/fences.rs#L46-L51"}
        ]
      }
    }
  }
}
```

The [fences()](code:crates/surface/src/fences.rs#L24-L65) iterator first splits the input into lines with offsets, preserving byte positions for each line. It then loops, checking each line to see if it opens a fence.

The [opening()](code:crates/surface/src/fences.rs#L75-L86) function returns the fence marker (backtick or tilde), its run length (≥3), and the trimmed info string—but only if the line starts with at most three spaces, begins with three or more identical fence characters, and (for backticks) contains no backticks in the info string. This last rule prevents ```` ```foo`bar```` from being recognized.

When an opening is found, the iterator searches forward for a closing line. The [closing()](code:crates/surface/src/fences.rs#L90-L94) function matches when a line has ≥ the opener's run length of the same marker character, followed by only whitespace. Crucially, closing lines inside another fence are just content—the [search happens linearly](code:crates/surface/src/fences.rs#L38) from the opening position, so nested fences never close the outer one.

If a closing line is found, the iterator yields a closed Fence with the body between the delimiters. If no closing is found, it yields an unclosed Fence with the body extending to the end of the text.

```json-render
{
  "root": "r",
  "elements": {
    "r": {
      "type": "Excerpt",
      "props": {
        "path": "crates/surface/src/fences.rs",
        "start": 74,
        "symbol": "opening",
        "lang": "rust",
        "caption": "Opening recognizes fence starts: ≥3 markers, no backticks in info string",
        "text": "/// `(marker, run length, trimmed info string)` if `line` opens a fence.\nfn opening(line: &str) -> Option<(u8, usize, &str)> {\n    let line = unindented(line)?;\n    let marker = *line.as_bytes().first()?;\n    if marker != b'\`' && marker != b'~' {\n        return None;\n    }\n    let len = line.bytes().take_while(|b| *b == marker).count();\n    let rest = &line[len..];\n    if len < 3 || (marker == b'\`' && rest.contains('\`')) {\n        return None;\n    }\n    Some((marker, len, rest.trim()))\n}"
      }
    }
  }
}
```

```json-render
{
  "root": "r",
  "elements": {
    "r": {
      "type": "Excerpt",
      "props": {
        "path": "crates/surface/src/fences.rs",
        "start": 89,
        "symbol": "closing",
        "lang": "rust",
        "caption": "Closing matches when run length ≥ opener and rest is whitespace",
        "text": "/// Whether `line` closes a fence opened by `len` × `marker`.\nfn closing(line: &str, marker: u8, len: usize) -> bool {\n    unindented(line).is_some_and(|l| {\n        let n = l.bytes().take_while(|b| *b == marker).count();\n        n >= len && l[n..].trim().is_empty()\n    })\n}"
      }
    }
  }
}
```

```json-render
{
  "root": "r",
  "elements": {
    "r": {
      "type": "Excerpt",
      "props": {
        "path": "crates/surface/src/fences.rs",
        "start": 38,
        "lang": "rust",
        "caption": "Linear search prevents nested fences from closing the outer one",
        "text": "            let close = (i + 1..lines.len()).find(|&j| closing(lines[j].1, marker, len));\n            return Some(match close {\n                Some(j) => {\n                    i = j + 1;\n                    let (cstart, cline) = lines[j];\n                    Fence {\n                        lang,\n                        body: &text[body_start..cstart],\n                        range: start..cstart + cline.trim_end_matches(['\\n', '\\r']).len(),\n                        closed: true,\n                    }\n                }\n                None => {\n                    i = lines.len();\n                    Fence {\n                        lang,\n                        body: &text[body_start..],\n                        range: start..text.len(),\n                        closed: false,\n                    }\n                }\n            });"
      }
    }
  }
}
```

## Decisions and trade-offs

**Why CommonMark's 3-space rule?** Allowing up to three leading spaces before a fence marker means fenced blocks can appear in list items and blockquotes without breaking. The unindented() check enforces this—four or more spaces makes the line a code block, not a fence.

**Why forbid backticks in the info string?** The spec rule `marker == b'\`' && rest.contains('`')```` in [opening()](code:crates/surface/src/fences.rs#L83) prevents ambiguity: `` ```foo````bar`` would make it unclear where the opening ends and the body begins. Tildes have no such restriction.

**Asymmetric delimiters.** The opening carries metadata (the language tag); the closing is just markers and whitespace. This means closing() can match any line that meets the length requirement, making fences resilient to typos in the closer.

**Linear search prevents nesting issues.** The find() from i+1 onward ensures that a closing marker inside a nested fence is never considered—only the first valid closing line after the current opening is matched. The test confirms ```md with `json-render inside yields one fence, not two.

## Checks

```json-render
{
  "root": "r",
  "elements": {
    "r": {
      "type": "Checklist",
      "props": {
        "items": [
          {"text": "ranges_cover_fence_lines_and_bodies_exclude_them", "done": true},
          {"text": "nested_fences_are_content", "done": true}
        ]
      }
    }
  }
}
```

The parser is tested by two unit tests in fences.rs: ranges_cover_fence_lines_and_bodies_exclude_them verifies that the byte ranges include delimiters but bodies exclude them, and handles both closed and unclosed fences. nested_fences_are_content confirms that a fence with four backticks correctly contains a three-backtick fence without treating it as a separate block.

Integration tests in lib.rs verify the public API, including check_ignores_fences_nested_in_another_block and check_reports_an_unclosed_fence.
