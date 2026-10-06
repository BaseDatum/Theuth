import { expect, test } from 'vitest'
import { segments } from './names'

test('names split after each underscore run; the underscores stay with the segment before them', () => {
  expect(segments('argocd_prod_dr__get_application_resource_tree')).toEqual(['argocd_', 'prod_', 'dr__', 'get_', 'application_', 'resource_', 'tree'])
  expect(segments('read-file')).toEqual(['read-file'])
  expect(segments('_private_')).toEqual(['_', 'private_'])
  expect(segments('')).toEqual([''])
})
