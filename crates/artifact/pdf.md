---
name: pdf
description: How `show` turns markdown into a PDF - front matter keys, what renders and what does not, page breaks and raw Typst blocks, when to switch to `source: typst`, and the compile errors seen most. Load before writing a `.pdf` artifact longer than a page or with layout needs.
---

# PDF documents with `show`

`show{content, name: "report.pdf"}` compiles `content` into a real PDF and hands back a picture
of page 1. Write markdown; the harness sets the page (A4, 11 pt serif, numbered pages, mono code
blocks, tables with rules, images shrunk to the text width). Look at the page-1 picture before you
tell the user the document is ready.

## Front matter

Optional YAML block at the top. Every key is optional; unknown keys are ignored.

```markdown
---
title: Quarterly summary      # title block on page 1
author: Ada Lovelace          # under the title
date: 2026-09-16              # default: today
paper: a4                     # or us-letter
toc: true                     # contents page after the title, then a page break
---
```

Without `title` there is no title block: start the document with a `#` heading instead.

## What renders

CommonMark plus GitHub tables, footnotes (`[^1]`), strikethrough, fenced code with a language
(highlighted), images, blockquotes, `---` rules, `<br>`, `<sup>`, `<sub>`. Smart punctuation is on:
`"quotes"`, `--`, `---`, `...` become typographic.

Images: `![caption](/absolute/path.png)` — an absolute path to a PNG/JPEG/GIF/SVG the harness can
read (a file you wrote, a screenshot). A relative path is taken from the working directory. A
missing file is a compile error, not a blank.

## What does not render

- **HTML** other than the tags above is dropped (its text stays). No `<div>`, `<table>`, `<style>`.
- **Math**: `$x^2$` is printed literally. For a formula use a raw Typst block: `<!--raw-typst $x^2 + y^2 = z^2$ -->`.
- **Task lists** `- [ ]` print as plain `[ ]` text.
- Very wide tables overflow the page; keep to about six columns or shorten the cells.

## Page breaks and raw Typst

An HTML comment starting with `raw-typst` is evaluated as Typst code, on its own line:

```markdown
<!--raw-typst #pagebreak() -->

<!--raw-typst #columns(2)[#lorem(120)] -->

<!--raw-typst #align(center)[#rect(inset: 8pt, stroke: 0.5pt)[Boxed note]] -->
```

The comment name must be exactly `raw-typst`: `<!--typst … -->` and `<!-- raw-typst … -->` are
ordinary comments and vanish silently.

## When to write Typst instead

Pass `source: "typst"` when the layout matters more than the text: multiple columns, a styled
table, a letterhead, exact spacing. `content` is then a whole Typst file; the harness adds nothing
(set your own page and fonts). Embedded fonts: Libertinus Serif, New Computer Modern, DejaVu Sans
Mono; system fonts also work.

```typst
#set page(paper: "a4", margin: 2cm, numbering: "1 / 1", columns: 2)
#set text(font: "Libertinus Serif", size: 10.5pt)
#show heading: set text(weight: "bold")
#place(top + center, scope: "parent", float: true)[
  #text(18pt, weight: "bold")[Two-column brief]
  #v(2mm)
  #text(fill: luma(90))[Ada Lovelace · 2026-09-16]
]
= Summary
#lorem(90)
= Numbers
#table(
  columns: (1fr, auto, auto),
  stroke: (x, y) => if y == 0 { (bottom: 0.7pt) } else { none },
  table.header([Region], [Requests], [p99]),
  [EU], [1 240 000], [180 ms],
  [US], [2 010 000], [210 ms],
)
= Next steps
#lorem(60)
```

## Errors you will see, and the fix

Errors come back as `line:col: message`, lines counted in your `content`.

1. `file not found (searched at /path/x.png)` — the image path is wrong or relative to the wrong
   directory. Use the absolute path the file was written to.
2. `unknown variable: tabel` (or any name) inside a `<!--raw-typst -->` block — a typo in a Typst
   function, or markdown syntax used inside Typst. Fix the block or drop it.
3. `unclosed delimiter` — a `[`, `(` or `{` in a raw Typst block has no partner, or the `-->` came
   too early. Close it, or move the code to `source: "typst"` where the whole file is Typst.
4. `front matter: …` — YAML that does not parse (unquoted `:` in a title, a stray tab). Quote the
   value.
