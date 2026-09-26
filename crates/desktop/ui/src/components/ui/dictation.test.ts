import { expect, test } from 'vitest'
import { splitLayout } from './dictation'

test('splitLayout moves box-sizing classes (with variants) to the wrapper and keeps field styling', () => {
  expect(splitLayout('h-8 w-40 font-mono flex-1 min-w-0 sm:col-span-2 pointer-coarse:h-11 mt-2 max-w-64 shadow-none')).toEqual([
    'w-40 flex-1 min-w-0 sm:col-span-2 mt-2 max-w-64',
    'h-8 font-mono pointer-coarse:h-11 shadow-none',
  ])
  expect(splitLayout(undefined)).toEqual(['', ''])
  // `md:text-sm` is not a margin; `min-h-[220px]` sizes the box.
  expect(splitLayout('md:text-sm min-h-[220px]')).toEqual(['min-h-[220px]', 'md:text-sm'])
})
