import { expect, test } from '@playwright/test'

const publicPages = [
  ['/', 'Plan what matters without scattering your attention.'],
  ['/privacy', 'Privacy Policy'],
  ['/terms', 'Terms of Service'],
  ['/delete-account', 'Delete your account'],
] as const

test('public pages have unique canonical URLs and useful HTML without JavaScript', async ({
  browser,
  baseURL,
}) => {
  const context = await browser.newContext({ baseURL, javaScriptEnabled: false })
  const page = await context.newPage()
  try {
    for (const [path, heading] of publicPages) {
      const response = await page.goto(path)
      expect(response!.status()).toBe(200)
      expect(response!.headers()['x-robots-tag'] ?? '').not.toContain('noindex')
      await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible()
      await expect(page.locator('link[rel="canonical"]')).toHaveCount(1)
      await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
        'href',
        `https://rakkio.com${path === '/' ? '/' : path}`,
      )
      await expect(page.locator('meta[name="robots"]')).toHaveAttribute('content', 'index, follow')
    }
  } finally {
    await context.close()
  }
})

test('crawl files have real formats and list only public canonical pages', async ({ request }) => {
  const robots = await request.get('/robots.txt')
  expect(robots.status()).toBe(200)
  expect(robots.headers()['content-type']).toContain('text/plain')
  expect(await robots.text()).toContain('Sitemap: https://rakkio.com/sitemap.xml')
  expect(await robots.text()).not.toContain('<html')
  const sitemap = await request.get('/sitemap.xml')
  expect(sitemap.status()).toBe(200)
  expect(sitemap.headers()['content-type']).toMatch(/(?:application|text)\/xml/)
  const xml = await sitemap.text()
  const locations = [...xml.matchAll(/<loc>(.*?)<\/loc>/g)].map((match) => match[1])
  expect(locations).toEqual(
    publicPages.map(([path]) => `https://rakkio.com${path === '/' ? '/' : path}`),
  )
})

test('legacy aliases redirect permanently without turning missing pages into homepages', async ({
  request,
}) => {
  for (const [from, to] of [
    ['/home', '/'],
    ['/home/', '/'],
    ['/privacy/', '/privacy'],
    ['/terms/', '/terms'],
    ['/delete-account.html', '/delete-account'],
  ]) {
    const response = await request.get(from!, { maxRedirects: 0 })
    expect(response.status()).toBe(308)
    expect(response.headers().location).toBe(to)
  }
  for (const path of ['/blog.html', '/missing-indexing-check']) {
    const response = await request.get(path, { maxRedirects: 0 })
    expect(response.status()).toBe(404)
    expect(await response.text()).toContain('Page not found')
    expect(await response.text()).not.toContain('rel="canonical"')
  }
})

test('private deep links remain available but cannot be indexed', async ({ request }) => {
  for (const path of [
    '/app.html', // Internal non-indexable client entry, never a sitemap page.
    '/inbox',
    '/calendar',
    '/projects',
    '/notes',
    '/files',
    '/settings',
    '/operations',
    '/groups',
    '/join?code=synthetic-test-only',
    '/groups/00000000-0000-0000-0000-000000000001/assignments',
  ]) {
    const response = await request.get(path)
    expect(response.status()).toBe(200)
    expect(response.headers()['x-robots-tag']).toBe('noindex, nofollow')
    expect(await response.text()).toContain('name="robots" content="noindex, nofollow"')
    expect(await response.text()).not.toContain('rel="canonical"')
    expect(await response.text()).not.toContain('synthetic-test-only')
  }
})

test('homepage stays visible after JavaScript starts while session discovery is slow', async ({
  page,
}) => {
  let release = () => {}
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  await page.route('**/api/v1/**', async (route) => {
    await gate
    await route.fulfill({
      status: 401,
      json: { error: { code: 'authentication_required', message: 'authentication is required' } },
    })
  })
  try {
    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await expect(page.locator('#app[data-v-app]')).toBeVisible()
    await expect(
      page.getByRole('heading', {
        name: 'Plan what matters without scattering your attention.',
        exact: true,
      }),
    ).toBeVisible()
  } finally {
    release()
    await page.unrouteAll({ behavior: 'wait' })
  }
})

test('public deletion and not-found layouts stay contained on narrow and wide screens', async ({
  page,
  isMobile,
}, testInfo) => {
  await page.setViewportSize({ width: isMobile ? 320 : 1280, height: 900 })
  await page.emulateMedia({ colorScheme: isMobile ? 'dark' : 'light' })
  await page.route('**/api/v1/**', (route) =>
    route.fulfill({
      status: 401,
      json: { error: { code: 'authentication_required', message: 'authentication is required' } },
    }),
  )
  for (const path of ['/delete-account', '/missing-indexing-check']) {
    await page.goto(path)
    await expect(page.locator('#app[data-v-app]')).toBeVisible()
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    if (path === '/delete-account' && isMobile) {
      await expect(page.locator('strong')).toHaveCSS('color', 'rgb(241, 245, 249)')
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await page.screenshot({
      path: testInfo.outputPath(path === '/delete-account' ? 'deletion.png' : 'not-found.png'),
      fullPage: true,
    })
  }
})

test('client navigation updates metadata and unknown routes stay on a real not-found page', async ({
  page,
}) => {
  await page.route('**/api/v1/**', (route) =>
    route.fulfill({
      status: 401,
      json: { error: { code: 'authentication_required', message: 'authentication is required' } },
    }),
  )
  await page.goto('/')
  await page.getByRole('link', { name: 'Privacy', exact: true }).first().click()
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
    'href',
    'https://rakkio.com/privacy',
  )
  await expect(page).toHaveTitle('Privacy Policy | rakkio')
  const response = await page.goto('/blog.html')
  expect(response!.status()).toBe(404)
  await expect(page).toHaveURL(/\/blog\.html$/)
  await expect(page.getByRole('heading', { name: 'Page not found', exact: true })).toBeVisible()
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute('content', 'noindex, nofollow')
  await expect(page.locator('link[rel="canonical"]')).toHaveCount(0)
})
