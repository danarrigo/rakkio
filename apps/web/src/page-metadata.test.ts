import { beforeEach, describe, expect, it } from 'vitest'
import { updatePageMetadata } from './page-metadata'
import { LEGACY_REDIRECTS, PUBLIC_PAGES, sitemapXml } from './site-pages'
import deployment from '../vercel.json'

describe('public indexing boundaries', () => {
  beforeEach(() => {
    document.head.innerHTML = '<meta name="description" content="initial">'
  })

  it('changes canonical metadata across public and private navigation without leaking queries', () => {
    updatePageMetadata('/', 'rakkio')
    expect(document.querySelector('link[rel="canonical"]')?.getAttribute('href')).toBe(
      'https://rakkio.com/',
    )
    updatePageMetadata('/settings', 'Settings | rakkio')
    expect(document.querySelector('link[rel="canonical"]')).toBeNull()
    expect(document.querySelector('meta[name="robots"]')?.getAttribute('content')).toBe(
      'noindex, nofollow',
    )
    updatePageMetadata('/privacy/', 'Privacy Policy | rakkio')
    expect(document.querySelectorAll('link[rel="canonical"]')).toHaveLength(1)
    expect(document.querySelector('link[rel="canonical"]')?.getAttribute('href')).toBe(
      'https://rakkio.com/privacy',
    )
    expect(document.querySelector('meta[name="robots"]')?.getAttribute('content')).toBe(
      'index, follow',
    )
    updatePageMetadata('/join', 'Group invitation | rakkio')
    expect(document.querySelector('link[rel="canonical"]')).toBeNull()
    expect(document.querySelector('meta[name="robots"]')?.getAttribute('content')).toBe(
      'noindex, nofollow',
    )
  })

  it('keeps deployment aliases aligned and never adds a blanket SPA rewrite', () => {
    expect(
      Object.fromEntries(
        deployment.redirects.map(({ source, destination, permanent }) => {
          expect(permanent).toBe(true)
          return [source, destination]
        }),
      ),
    ).toEqual(LEGACY_REDIRECTS)
    expect(
      deployment.rewrites.every(({ destination }) =>
        ['/app.html', '/privacy.html', '/terms.html', '/delete-account.html'].includes(destination),
      ),
    ).toBe(true)
    const xml = sitemapXml()
    expect([...xml.matchAll(/<loc>/g)]).toHaveLength(PUBLIC_PAGES.length)
    expect(xml).not.toMatch(/settings|groups|join|api\.|\.html/)
    expect(xml).not.toMatch(/<loc>[^<]*\?/)
  })
})
