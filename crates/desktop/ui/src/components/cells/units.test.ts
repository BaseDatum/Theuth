import { expect, test } from 'vitest'
import { commandUnits, unitMode, unitOf } from './units'

const sh = (...lines: string[]) => commandUnits(lines.join('\n'), 'bash')

test("Sky's soft-wrapped gh command is one logical line: one unit, run as that line", () => {
  const code = `gh run list --repo sky/sre-release --workflow release.yml --limit 20 --json databaseId,status,conclusion,headBranch,createdAt --jq '.[] | select(.conclusion != "success") | [.databaseId, .headBranch, .createdAt] | @tsv'`
  expect(commandUnits(code, 'bash')).toEqual([[1, 1]])
  expect(unitMode(code, unitOf(commandUnits(code, 'bash'), 1))).toEqual({ type: 'line', line: 1 })
})

test('shell: plain lines are their own units', () => {
  expect(sh('echo one', 'echo two', '', 'ls')).toEqual([[1, 1], [2, 2], [3, 3], [4, 4]])
})

test('shell: backslash continuation, trailing pipe / &&, and an unclosed quote join lines', () => {
  expect(sh('gh run list \\', '  --limit 20 \\', '  --json id', 'echo after')).toEqual([[1, 3], [4, 4]])
  expect(sh('ls |', '  wc -l', 'make &&', '  make test', 'echo x')).toEqual([[1, 2], [3, 4], [5, 5]])
  expect(sh("gh api --jq '.[]", "  | .name'", 'echo "a', 'b"', 'echo x')).toEqual([[1, 2], [3, 4], [5, 5]])
  // An escaped quote opens nothing; a quote inside the other kind neither.
  expect(sh("echo it\\'s", 'echo "don\'t"', 'echo x')).toEqual([[1, 1], [2, 2], [3, 3]])
})

test('shell: a heredoc runs to its terminator (also <<-, quoted tags); <<< is not one', () => {
  expect(sh("cat <<'EOF' > /tmp/x", 'a $b', 'EOF', 'echo after')).toEqual([[1, 3], [4, 4]])
  expect(sh('cat <<-END', '\tbody', '\tEND', 'echo after')).toEqual([[1, 3], [4, 4]])
  expect(sh('grep x <<< "$v"', 'echo after')).toEqual([[1, 1], [2, 2]])
})

test('shell: brackets and if/for/while/case blocks are one unit; comments do not open anything', () => {
  expect(sh('f() {', '  echo in', '}', 'echo after')).toEqual([[1, 3], [4, 4]])
  expect(sh('(', '  cd /tmp; ls', ')', 'x')).toEqual([[1, 3], [4, 4]])
  expect(sh('if [ -f x ]; then', '  echo y', 'else', '  echo n', 'fi', 'x')).toEqual([[1, 5], [6, 6]])
  expect(sh('for f in *; do', '  echo "$f"', 'done', 'while read l; do', '  echo $l', 'done < f', 'x')).toEqual([[1, 3], [4, 6], [7, 7]])
  expect(sh('case $x in', '  a) echo a ;;', '  *) echo z ;;', 'esac', 'x')).toEqual([[1, 4], [5, 5]])
  expect(sh('echo hi # if ( {', 'echo ${HOME}', 'x')).toEqual([[1, 1], [2, 2], [3, 3]])
  expect(sh('for f in a; do echo $f; done', 'x')).toEqual([[1, 1], [2, 2]])
})

test('CELLAFTER F5: if/fi, do/done, case/esac count only as a command’s first word — never as an argument, quoted or commented', () => {
  expect(sh('for f in *; do echo done; done', 'x')).toEqual([[1, 1], [2, 2]])
  expect(sh('if grep -q fi x; then echo ok; fi', 'x')).toEqual([[1, 1], [2, 2]])
  // Inside a loop, `done` as an argument, in quotes or in a comment keeps the loop open.
  expect(sh('for f in *; do', '  echo done', '  grep fi "$f" # done here', "  echo 'esac' \"fi\"", 'done', 'x')).toEqual([[1, 5], [6, 6]])
  expect(sh('while true; do', '  # done', '  sleep 1', 'done', 'x')).toEqual([[1, 4], [5, 5]])
  // A keyword after a pipe, `&&` or `(` is a command's first word again.
  expect(sh('ls | while read l; do', '  echo $l', 'done', 'x')).toEqual([[1, 3], [4, 4]])
  expect(sh('true && if x; then y; fi', 'x')).toEqual([[1, 1], [2, 2]])
  // A `\` continuation's next line is still the same command's arguments.
  expect(sh('for f in a b \\', '  done; do', '  echo $f', 'done', 'x')).toEqual([[1, 4], [5, 5]])
})

test('python: open brackets and a `:` header with its indented body (and its clauses) are one unit', () => {
  const py = (...lines: string[]) => commandUnits(lines.join('\n'), 'python')
  expect(py('x = [', '  1,', '  2,', ']', 'print(x)')).toEqual([[1, 4], [5, 5]])
  expect(py('for i in range(3):', '    print(i)', '', '    print(i * 2)', 'print("done")')).toEqual([[1, 4], [5, 5]])
  expect(py('try:', '    f()', 'except Exception:', '    g()', 'finally:', '    h()', 'x')).toEqual([[1, 6], [7, 7]])
  expect(py('@cache', 'def f(x):', '    return x', '', 'f(1)')).toEqual([[1, 3], [4, 4], [5, 5]])
  expect(py('if x: y()', 'z = {"a": 1}', 's = """', 'text:', '"""')).toEqual([[1, 1], [2, 2], [3, 5]])
  expect(py('total = 1 + \\', '    2')).toEqual([[1, 2]])
})

test('pwsh: a backtick continuation and an open brace are one unit', () => {
  const ps = (...lines: string[]) => commandUnits(lines.join('\n'), 'pwsh')
  expect(ps('Get-ChildItem `', '  -Recurse', 'Write-Host x')).toEqual([[1, 2], [3, 3]])
  expect(ps('foreach ($f in $x) {', '  $f', '}', 'y')).toEqual([[1, 3], [4, 4]])
})

test('CELLAFTER S5: ANSI-C quotes, python comment edge cases, pwsh trailing | and ,', () => {
  // `$'…'` takes `\'` as an escaped quote: the line closes, nothing after it joins.
  expect(sh("echo $'it\\'s'", 'echo next')).toEqual([[1, 1], [2, 2]])
  expect(sh("printf $'a\\tb\\n'", 'echo next')).toEqual([[1, 1], [2, 2]])
  const py = (...lines: string[]) => commandUnits(lines.join('\n'), 'python')
  // A comment ending in `\` joins nothing.
  expect(py('x = 1  # see C:\\', 'y = 2')).toEqual([[1, 1], [2, 2]])
  // A column-0 comment inside an indented body keeps the block whole.
  expect(py('for i in range(3):', '    a = i', '# halfway', '    print(a)', 'done = 1')).toEqual([[1, 4], [5, 5]])
  const ps = (...lines: string[]) => commandUnits(lines.join('\n'), 'pwsh')
  expect(ps('Get-Process |', '  Sort-Object CPU', 'Write-Host x')).toEqual([[1, 2], [3, 3]])
  expect(ps('$xs = 1,', '  2, 3', 'Write-Host x')).toEqual([[1, 2], [3, 3]])
  expect(ps('Write-Host a # ends with |', 'Write-Host b')).toEqual([[1, 1], [2, 2]])
})

test('a multi-line unit runs as a selection of exactly its lines (UTF-8 byte offsets)', () => {
  const code = 'echo é\ncurl x \\\n  -o y\necho z'
  const mode = unitMode(code, unitOf(commandUnits(code, 'bash'), 3))
  expect(mode).toEqual({ type: 'selection', from: 8, to: 8 + 'curl x \\\n  -o y'.length })
  expect(unitOf([[1, 2]], 5)).toEqual([5, 5])
})
