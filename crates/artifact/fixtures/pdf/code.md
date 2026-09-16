# Code

Inline `let x = 1;` and a fenced block with a language:

```rust
fn main() {
    let greeting = "hello";
    println!("{greeting}, world");
}
```

A block without a language, containing characters that are special in Typst: `#`, `$`, `_`, `*`, `<`, `>`, `@`, `\`:

```
#let x = $y$ _z_ *w* <a> @b \n
```

An indented code block:

    indented code
    second line

And a YAML block:

```yaml
artifact:
  enabled: true
  fonts: []
```
