export const SITE_ORIGIN = 'https://rakkio.com'

export const PUBLIC_PAGES = [
  {
    path: '/',
    title: 'rakkio',
    description:
      'rakkio combines tasks, projects, notes, and calendars into one calm daily command center.',
  },
  {
    path: '/privacy',
    title: 'Privacy Policy | rakkio',
    description: 'How rakkio collects, uses, stores, and protects personal data.',
  },
  {
    path: '/terms',
    title: 'Terms of Service | rakkio',
    description: 'The rules for accessing and using the hosted rakkio service.',
  },
  {
    path: '/delete-account',
    title: 'Delete your account | rakkio',
    description: 'How to delete your rakkio account and request removal of your personal data.',
  },
]

export const LEGACY_REDIRECTS: Readonly<Record<string, string>> = {
  '/home': '/',
  '/home/': '/',
  '/index.html': '/',
  '/privacy/': '/privacy',
  '/privacy.html': '/privacy',
  '/terms/': '/terms',
  '/terms.html': '/terms',
  '/delete-account/': '/delete-account',
  '/delete-account.html': '/delete-account',
}

export function isWorkspacePath(path: string) {
  return (
    /^\/(?:inbox|calendar|projects|notes|operations|files|groups|join|settings)\/?$/.test(path) ||
    /^\/groups\/[^/]+\/assignments\/?$/.test(path) ||
    path === '/app.html'
  )
}

export function robotsText() {
  // Crawling must remain allowed so Google can observe private pages' noindex.
  return `User-agent: *\nAllow: /\n\nSitemap: ${SITE_ORIGIN}/sitemap.xml\n`
}

export function sitemapXml() {
  return `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${PUBLIC_PAGES.map(({ path }) => `  <url><loc>${SITE_ORIGIN}${path}</loc></url>`).join('\n')}\n</urlset>\n`
}
