import { expect, test } from 'vitest'
import { slugify, templateBody } from './meeting-templates'

test('templateBody: front matter only, scalars JSON-quoted, instruction/required only when set', () => {
  const body = templateBody({
    name: 'One-on-one: "weekly"',
    description: 'Manager / report check-in #1',
    sections: [
      { title: 'Summary', instruction: 'at most 120 words', required: false },
      { title: 'Action items', instruction: '- [ ] Action — Owner (due if said)', required: true },
      { title: 'Open questions', instruction: null, required: false },
    ],
  })
  expect(body).toBe(
    [
      '---',
      'name: "One-on-one: \\"weekly\\""',
      'description: "Manager / report check-in #1"',
      'sections:',
      '  - title: "Summary"',
      '    instruction: "at most 120 words"',
      '  - title: "Action items"',
      '    instruction: "- [ ] Action — Owner (due if said)"',
      '    required: true',
      '  - title: "Open questions"',
      '---',
      '',
    ].join('\n'),
  )
})

test('slugify: lowercase ASCII words joined by -, accents and punctuation dropped, empty when nothing is left', () => {
  expect(slugify(' Customer Call ')).toBe('customer-call')
  expect(slugify('Café résumé!')).toBe('cafe-resume')
  expect(slugify('—')).toBe('')
})
