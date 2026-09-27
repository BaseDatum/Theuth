// Glue around cmarker for markdown → PDF (design doc 35 §4.1): page, fonts, title block, contents.
// Not a layout engine; a second theme is a second file. `sys.inputs.meta` = {title, author, date,
// paper, toc, font, body}: front matter as parsed by pdf.rs, `body` the markdown minus the front matter.
#import "cmarker/lib.typ": render

#let meta = sys.inputs.meta

#set page(paper: meta.paper, numbering: "1 / 1", margin: 2.2cm)
#set text(font: (meta.font, "Libertinus Serif", "New Computer Modern"), size: 11pt)
#set document(title: meta.title)
#show raw: set text(font: "DejaVu Sans Mono", size: 9pt)
#show raw.where(block: true): block.with(width: 100%, fill: luma(245), inset: 7pt, radius: 2pt)
#set table(stroke: 0.5pt + luma(170), inset: 6pt)
#show table.cell.where(y: 0): strong
// Images wider than the text block shrink to it; smaller ones keep their natural size.
#show image: it => layout(size => {
  if measure(it).width > size.width { set image(width: 100%); it } else { it }
})

#if meta.title != none [
  #align(center)[
    #text(20pt, weight: "bold")[#meta.title]
    #v(2mm)
    #text(fill: luma(90))[#meta.author #h(1em) #meta.date]
  ]
  #v(8mm)
]
#if meta.toc [
  #outline(depth: 2)
  #pagebreak()
]

// cmarker resolves image paths relative to its own file; the `image` override moves that to the
// project root, where pdf.rs's resolver maps absolute paths to the disk and relative ones to cwd.
#render(
  meta.body,
  smart-punctuation: true,
  math: none,
  scope: (image: (source, ..args) => image(source, ..args)),
)
