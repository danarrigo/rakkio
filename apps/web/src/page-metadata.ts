import { PUBLIC_PAGES, SITE_ORIGIN } from './site-pages'

export function updatePageMetadata(path: string, fallbackTitle: string) {
  const page = PUBLIC_PAGES.find((entry) => entry.path === (path.replace(/\/$/, '') || '/'))
  document.title = page?.title ?? fallbackTitle
  let robots = document.querySelector<HTMLMetaElement>('meta[name="robots"]')
  if (!robots) {
    robots = document.createElement('meta')
    robots.name = 'robots'
    document.head.append(robots)
  }
  robots.content = page ? 'index, follow' : 'noindex, nofollow'
  for (const existing of document.querySelectorAll('link[rel="canonical"]')) existing.remove()
  if (page) {
    const canonical = document.createElement('link')
    canonical.rel = 'canonical'
    canonical.href = `${SITE_ORIGIN}${page.path}`
    document.head.append(canonical)
  }
  const description = document.querySelector<HTMLMetaElement>('meta[name="description"]')
  if (description) description.content = page?.description ?? PUBLIC_PAGES[0]!.description
}
