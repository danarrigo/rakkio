import { readFile, writeFile } from 'node:fs/promises'
import { createServer } from 'vite'

const dist = new URL('../dist/', import.meta.url)
const template = await readFile(new URL('index.html', dist), 'utf8')
const manifest = JSON.parse(await readFile(new URL('.vite/manifest.json', dist), 'utf8'))
const server = await createServer({ server: { middlewareMode: true }, appType: 'custom' })
const sourceByPath = {
  '/privacy': 'src/views/PrivacyView.vue',
  '/terms': 'src/views/TermsView.vue',
  '/delete-account': 'src/views/DeleteAccountView.vue',
  '/404': 'src/views/NotFoundView.vue',
}
const escape = (value) =>
  value.replace(
    /[&<>"']/g,
    (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char],
  )

function styles(path) {
  const found = new Set()
  const visited = new Set()
  function visit(key) {
    if (!key || visited.has(key)) return
    visited.add(key)
    const entry = manifest[key]
    for (const css of entry?.css ?? []) found.add(css)
    for (const imported of entry?.imports ?? []) visit(imported)
  }
  visit('index.html')
  visit(sourceByPath[path])
  return [...found]
    .filter((css) => !template.includes(`/${css}`))
    .map((css) => `<link rel="stylesheet" href="/${escape(css)}">`)
    .join('\n')
}

try {
  const { PUBLIC_PAGES, SITE_ORIGIN, renderPublicPage, robotsText, sitemapXml } =
    await server.ssrLoadModule('/src/entry-public.ts')
  // Keep an empty, non-indexable entry for authenticated client routes. No
  // account/session data is rendered or serialized by this build step.
  await writeFile(new URL('app.html', dist), template)
  for (const page of [
    ...PUBLIC_PAGES,
    {
      path: '/404',
      title: 'Page not found | rakkio',
      description: 'This address does not point to a current rakkio page.',
    },
  ]) {
    const indexable = page.path !== '/404'
    const body = await renderPublicPage(page.path)
    let html = template
      .replace(/<title>.*?<\/title>/s, `<title>${escape(page.title)}</title>`)
      .replace(
        /<meta\s+name="description"\s+content="[^"]*"\s*\/?\s*>/s,
        `<meta name="description" content="${escape(page.description)}">`,
      )
      .replace('<div id="app"></div>', `<div id="app">${body}</div>`)
    if (indexable) {
      html = html
        .replace(
          'name="robots" content="noindex, nofollow"',
          'name="robots" content="index, follow"',
        )
        .replace('</head>', `<link rel="canonical" href="${SITE_ORIGIN}${page.path}">\n</head>`)
    }
    html = html.replace('</head>', `${styles(page.path)}\n</head>`)
    await writeFile(
      new URL(page.path === '/' ? 'index.html' : `${page.path.slice(1)}.html`, dist),
      html,
    )
  }
  await writeFile(new URL('robots.txt', dist), robotsText())
  await writeFile(new URL('sitemap.xml', dist), sitemapXml())
} finally {
  await server.close()
}
