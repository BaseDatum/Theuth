/**
 * The synthetic corpus (no files read) that `golden-inproc.ts` replays against crates/document-js/tests/golden.json.
 */
export type Input = { name: string; text: string; frontmatter: boolean };

export function syntheticCorpus(): Input[] {
  const corpus: Input[] = [];
  const add = (name: string, text: string, frontmatter = text.startsWith("---\n")) => corpus.push({ name, text, frontmatter });

// convert.test.ts round-trip cases (verbatim inputs).
const cases: [string, string][] = [
  ["test:paragraph", "Plain **bold** *italic* ~~strike~~ `code` [link](https://x.y/)."],
  ["test:headings", "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six"],
  ["test:bullet list", "- a\n- b\n  - nested"],
  ["test:numbered list", "1. one\n2. two"],
  ["test:check list", "- [ ] todo\n- [x] done"],
  ["test:code block", "```rust\nfn main() {}\n```"],
  ["test:mermaid", "```mermaid\nflowchart LR\n  A --> B\n```"],
  ["test:json-render", '```json-render\n{"root":"a"}\n```'],
  ["test:table", "| a | b |\n| --- | --- |\n| 1 | 2 |"],
  ["test:quote", "> quoted"],
  ["test:divider", "para\n\n---\n\npara2"],
  ["test:code: link", "See [parse()](code:crates/surface/src/lib.rs#L71-L84) and [x](theuth://notes/1)."],
  ["test:all types", "# T\n\npara\n\n- a\n\n1. b\n\n- [ ] c\n\n```js\nx\n```\n\n| a |\n| - |\n| 1 |\n\n> q\n\n---\n"],
];
for (const [n, t] of cases) add(n, t, false);
add(
  "test:frontmatter header",
  `---\ntype: fact\ntitle: theuth's Rust toolchain lives under rustup, not on PATH\ndescription: "cargo/rustc are at ~/.rustup: homebrew's rustup does not proxy them"\ntags:\n  - rust\n  - toolchain\nscope: /Users/Sky.Moore/Develop/theuth\nsource: harvest\ntimestamp: 2026-09-10T09:41:03Z\nextra_key: something the store never saw\n---\n\nBody para.\n`,
);
add(
  "test:outline golden",
  "---\ntype: project\ntitle: wth apps\ntags:\n  - wth\n  - openui\n---\n## Decisions\n\n- app = artifact\n  - nested child\n\n```rust\npub struct A {}\nfn b() {}\n```\n\n| step | crates | done when |\n| --- | --- | --- |\n| 1 | a | b |\n| 2 | c | d |\n",
);

// Synthetic edge cases.
add("syn:deep nesting", "- l1\n  - l2\n    - l3\n      - l4\n        1. n5\n        2. n5b\n  - l2b\n- [ ] check\n  - [x] nested check\n");
add("syn:marks", "***bold italic*** and **bold `code` inside** and ~~*strike italic*~~ and <u>underline?</u> \\*escaped\\* a_b_c snake_case_word\n\nline one  \nhard break\n");
add("syn:links", "[http](https://example.com/a?b=c&d=e) [rel](./docs/x.md) [code](code:crates/a/src/b.rs#L1-L9) [theuth](theuth://notes/42#blk) [mail](mailto:a@b.c) [bad](javascript:alert(1)) [ftp](ftp://h/f) <https://auto.link/>\n");
add("syn:quarto fence", "```python\n#| label: fig-x\n#| echo: false\nimport numpy as np\nprint(np.pi)\n```\n\nText after.\n");
add("syn:json-render big", '```json-render\n{\n  "root": "card",\n  "elements": {\n    "card": {"type": "Card", "props": {"title": "T"}, "children": ["t"]},\n    "t": {"type": "Text", "props": {"text": "hi <b>"}}\n  }\n}\n```\n');
add("syn:table marks", "| **h1** | `h2` | [h3](https://x.y) |\n| :--- | :---: | ---: |\n| *a* | b \\| pipe | ~~c~~ |\n| | empty | cells |\n");
add("syn:unicode", "# Ünïcödé — “quotes” ‘single’ … 🚀\n\nCJK 漢字かな 한국어 עברית العربية\n\n- • bullet char\n- ◦ another\n");
add("syn:html-ish", "Inline <span>html</span> & entities &amp; &lt;tag&gt; &copy; &#x1F600;\n\n<div>block html</div>\n");
add("syn:frontmatter only", "---\ntitle: empty body\ntags: [a, b]\n---\n");
add("syn:empty", "");
add("syn:code fences mixed", "~~~\ntilde fence\n~~~\n\n```\nno lang\n  indented\n\ttab\n```\n\n    indented code block\n\n```ts title=\"x.ts\"\nconst a = 1;\n```\n");
add("syn:numbered start", "3. three\n4. four\n\n10. ten\n");
add("syn:quote nested", "> level one\n> > level two\n>\n> - list in quote\n");
  return corpus;
}
