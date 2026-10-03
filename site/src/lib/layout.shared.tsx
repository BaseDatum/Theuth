import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared'

export const SITE = 'https://theuth.io'

/** Navbar/sidebar chrome shared by every docs page: wordmark home link, Home, Docs. No source link: the repository is
 *  not publicly reachable (GitHub API 404s unauthenticated); add `githubUrl` here when it is. */
export const baseOptions: BaseLayoutProps = {
  nav: {
    title: <span className="font-display text-[22px] leading-none tracking-[0.04em]">theuth</span>,
    url: '/',
  },
  links: [
    { text: 'Home', url: '/', active: 'url' },
    { text: 'Docs', url: '/docs', active: 'nested-url' },
  ],
  // Theme follows the OS (styles.css tokens are `prefers-color-scheme` driven); no manual toggle anywhere on the site.
  themeSwitch: { enabled: false },
}
