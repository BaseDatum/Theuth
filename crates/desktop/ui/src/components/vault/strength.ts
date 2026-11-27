/**
 * 225: a dependency-free estimate of how hard a master password is to guess, for the meter under the password fields.
 * Bits from the character pool per character, minus what an attacker's dictionary gets for free: common words (with
 * the usual digit-for-letter swaps), repeats, runs (`abc`, `321`, `qwer`) and a repeated chunk (`abcabcabc`).
 */

/** 225: the shortest master password the engine accepts (`envelope::MIN_PASSWORD`). */
export const MIN_PASSWORD = 12

export type Strength = {
  /** 0 (too short or very weak) to 4 (strong). */
  score: 0 | 1 | 2 | 3 | 4
  label: string
  hint: string
}

const LABELS = ['Very weak', 'Weak', 'Fair', 'Good', 'Strong'] as const

/** Longest first, so `password` wins over `pass`. Lowercase, after `LEET`. */
const WORDS = [
  'changeme', 'password', 'iloveyou', 'princess', 'sunshine', 'football', 'baseball', 'superman', 'whatever', 'starwars',
  'computer', 'welcome', 'letmein', 'default', 'freedom', 'monkey', 'dragon', 'shadow', 'master', 'secret', 'qwerty',
  'summer', 'winter', 'spring', 'autumn', 'theuth', 'admin', 'login', 'vault', 'hello', 'pass', 'love', 'test',
].sort((a, b) => b.length - a.length)

const LEET: Record<string, string> = { '0': 'o', '1': 'l', '3': 'e', '4': 'a', '5': 's', '7': 't', '@': 'a', '$': 's', '!': 'i' }

const ROWS = ['`1234567890-=', 'qwertyuiop[]', "asdfghjkl;'", 'zxcvbnm,./']

/** Bits an attacker pays for a word from a short list of common ones. */
const WORD_BITS = 10
/** Bits for a character that repeats or continues a run. */
const PREDICTABLE_BITS = 1

/** How `b` follows `a` when it is predictable: the same step on the alphabet or a keyboard row, or `null`. */
function step(a: string, b: string): string | null {
  const d = b.charCodeAt(0) - a.charCodeAt(0)
  if (Math.abs(d) <= 1) return `c${d}`
  for (const row of ROWS) {
    const i = row.indexOf(a)
    const j = row.indexOf(b)
    if (i >= 0 && j >= 0 && Math.abs(j - i) === 1) return `k${j - i}`
  }
  return null
}

/** The size of the alphabet `pw` draws from. */
function pool(pw: string): number {
  let n = 0
  if (/[a-z]/.test(pw)) n += 26
  if (/[A-Z]/.test(pw)) n += 26
  if (/[0-9]/.test(pw)) n += 10
  if (/[^a-zA-Z0-9]/.test(pw)) n += 33
  // Letters outside ASCII (accents, other scripts) widen the pool further.
  if (/[^\x00-\x7f]/.test(pw)) n += 100
  return n
}

/** The length of the smallest unit `chars` is a repetition of, or of `chars` itself. */
function period(chars: readonly string[]): number {
  for (let p = 1; p <= chars.length / 2; p++) if (chars.length % p === 0 && chars.every((c, i) => c === chars[i % p])) return p
  return chars.length
}

export function strength(password: string): Strength {
  const chars = [...password]
  if (chars.length < MIN_PASSWORD) {
    const more = MIN_PASSWORD - chars.length
    return { score: 0, label: 'Too short', hint: `Use at least ${MIN_PASSWORD} characters (${more} more)` }
  }
  const unit = chars.slice(0, period(chars))
  const lower = unit.map((c) => c.toLowerCase())
  const leet = lower.map((c) => LEET[c] ?? c)
  const perChar = Math.log2(pool(password))

  // Common words: each costs WORD_BITS however long, and its characters nothing more.
  const inWord = new Array<boolean>(unit.length).fill(false)
  let words = 0
  for (const word of WORDS) {
    for (let at = 0; at + word.length <= unit.length; at++) {
      if (leet.slice(at, at + word.length).join('') !== word || inWord.slice(at, at + word.length).some(Boolean)) continue
      inWord.fill(true, at, at + word.length)
      words++
    }
  }

  // A character that takes the same step as the one before it (`aaa`, `abc`, `321`, `qwe`) is nearly free.
  let bits = words * WORD_BITS
  let predictable = 0
  for (let i = 0; i < unit.length; i++) {
    if (inWord[i]) continue
    const prev = i >= 2 ? step(lower[i - 2]!, lower[i - 1]!) : null
    if (prev !== null && step(lower[i - 1]!, lower[i]!) === prev) {
      predictable++
      bits += PREDICTABLE_BITS
    } else bits += perChar
  }
  // A repeated chunk: the attacker guesses the chunk and the count.
  const repeated = unit.length < chars.length
  if (repeated) bits += 2

  const score = bits < 35 ? 0 : bits < 50 ? 1 : bits < 65 ? 2 : bits < 80 ? 3 : 4
  const hint =
    score === 4
      ? 'Strong password'
      : words > 0
        ? 'Avoid common words and names, or add more unrelated words'
        : predictable > 0 || repeated
          ? 'Avoid repeats and runs like aaa, abc or 1234'
          : score === 3
            ? 'Good; a few more characters make it strong'
            : 'Make it longer, or mix in capitals, digits and symbols'
  return { score, label: LABELS[score], hint }
}
