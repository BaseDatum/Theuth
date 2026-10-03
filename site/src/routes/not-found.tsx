import { HomeLayout } from 'fumadocs-ui/layouts/home'
import type { MetaFunction } from 'react-router'
import { NotFound } from '../components/not-found'
import { baseOptions } from '../lib/layout.shared'

export const meta: MetaFunction = () => [{ title: 'Not found · theuth' }, { name: 'robots', content: 'noindex' }]

export default function NotFoundRoute() {
  return (
    <HomeLayout {...baseOptions}>
      <NotFound />
    </HomeLayout>
  )
}
