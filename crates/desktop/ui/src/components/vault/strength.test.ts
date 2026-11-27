import { expect, test } from 'vitest'
import { MIN_PASSWORD, strength } from './strength'

test('225: below MIN_PASSWORD is too short whatever it holds, and says how many more', () => {
  expect(MIN_PASSWORD).toBe(12)
  expect(strength('')).toEqual({ score: 0, label: 'Too short', hint: 'Use at least 12 characters (12 more)' })
  expect(strength('Xk7#Qm2$vL9')).toMatchObject({ score: 0, label: 'Too short', hint: 'Use at least 12 characters (1 more)' })
})

test('repeats, runs and repeated chunks score very weak', () => {
  for (const pw of ['aaaaaaaaaaaa', 'abcdefghijkl', '987654321098', 'qwertyuiop[]', 'abcabcabcabc', 'xY1!xY1!xY1!']) {
    const s = strength(pw)
    expect(s.score, pw).toBeLessThanOrEqual(1)
    expect(s.hint, pw).not.toBe('Strong password')
  }
  expect(strength('aaaaaaaaaaaa').hint).toBe('Avoid repeats and runs like aaa, abc or 1234')
})

test('common words count as one guess, digit swaps included', () => {
  expect(strength('password1234')).toMatchObject({ score: 0, hint: 'Avoid common words and names, or add more unrelated words' })
  expect(strength('P@ssw0rd2024').score).toBeLessThanOrEqual(1)
  expect(strength('iloveyousunshine').score).toBeLessThanOrEqual(1)
})

test('random characters and long passphrases score well; longer is never weaker', () => {
  expect(strength('Xk7#Qm2$vL9p').score).toBe(3)
  expect(strength('Xk7#Qm2$vL9pRw4!')).toEqual({ score: 4, label: 'Strong', hint: 'Strong password' })
  expect(strength('correct horse battery staple').score).toBe(4)
  const base = 'Xk7#Qm2$vL9p'
  for (let n = 0; n < 6; n++) expect(strength(base + 'Rw4!Zt'.slice(0, n + 1)).score).toBeGreaterThanOrEqual(strength(base).score)
})

test('every result has a label matching its score', () => {
  const labels = ['Very weak', 'Weak', 'Fair', 'Good', 'Strong']
  for (const pw of ['aaaaaaaaaaaa', 'lowercaseonly', 'Xk7#Qm2$vL9p', 'correct horse battery staple']) {
    const s = strength(pw)
    expect(s.label).toBe(labels[s.score])
  }
})
