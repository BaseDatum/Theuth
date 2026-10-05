import { expect, test } from 'vitest'
import { parseUnifiedDiff } from './diff'

const TWO = `diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -10,4 +10,5 @@ fn main() {
 let a = 1;
-let b = 2;
+let b = 3;
+let c = 4;
 println!("{a}");
diff --git a/README.md b/README.md
new file mode 100644
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Hi
+text
\\ No newline at end of file
`

test('parses files, hunks, counts and line numbers', () => {
  const files = parseUnifiedDiff(TWO)
  expect(files.map((f) => [f.path, f.added, f.removed])).toEqual([
    ['src/a.rs', 2, 1],
    ['README.md', 2, 0],
  ])
  const [a, readme] = files
  expect(a?.hunks[0]?.header).toBe('@@ -10,4 +10,5 @@ fn main() {')
  expect(a?.hunks[0]?.rows.map((r) => [r.op, r.old, r.new])).toEqual([
    [' ', 10, 10],
    ['-', 11, null],
    ['+', null, 11],
    ['+', null, 12],
    [' ', 12, 13],
  ])
  expect(readme?.hunks[0]?.rows.at(-1)).toEqual({ op: ' ', text: '\\ No newline at end of file', old: null, new: null })
})

test('bare ---/+++ pairs without diff --git still split files; a rename takes the new name', () => {
  const files = parseUnifiedDiff(`--- a/old.ts\n+++ b/new.ts\n@@ -1 +1 @@\n-x\n+y\n--- a/z.ts\n+++ b/z.ts\n@@ -1 +1 @@\n-p\n+q\n`)
  expect(files.map((f) => f.path)).toEqual(['new.ts', 'z.ts'])
})

test('no hunk → no files, so the renderer falls back to the raw text', () => {
  expect(parseUnifiedDiff('not a diff\njust text')).toEqual([])
  expect(parseUnifiedDiff('')).toEqual([])
})
