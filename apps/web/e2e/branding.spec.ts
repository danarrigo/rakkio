import { readFileSync } from 'node:fs'
import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'

async function mockWorkspace(page: Page, signedIn: boolean, sidebarVisible = true) {
  await page.route('**/api/v1/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '')
    if (!signedIn) {
      await route.fulfill({ status: 401, json: { error: 'unauthorized' } })
      return
    }
    let body: unknown = path === '/saved-task-views' ? [] : { items: [] }
    if (path === '/session')
      body = {
        csrf_token: 'mock-csrf',
        user: { id: 'user-1', email: 'planner@example.test', display_name: 'Planner' },
      }
    else if (path === '/settings')
      body = {
        theme: 'system',
        automatic_daily_review: false,
        sidebar_visible: sidebarVisible,
        sync_conflict_policy: 'ask',
        version: 1,
      }
    else if (path.startsWith('/daily-plans/')) body = { focus_tasks: [] }
    else if (path === '/operations/capability') body = true
    await route.fulfill({ json: body })
  })
}

test.beforeEach(async ({ page }) => {
  page.on('pageerror', (error) => {
    throw error
  })
  // No backend or external font/provider requests are needed by these checks.
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url())
    if (url.origin !== 'http://127.0.0.1:5173') {
      await route.abort()
      throw new Error(`Unexpected external request: ${url.origin}`)
    }
    await route.fallback()
  })
})

test('raster favicon fallbacks are declared, served as images, and bypass the SPA rewrite', async ({
  page,
  request,
}) => {
  await mockWorkspace(page, false)
  await page.goto('/')
  await expect(page.locator('link[rel="icon"][type="image/png"]')).toHaveAttribute(
    'href',
    '/favicon.png',
  )
  await expect(page.locator('link[rel="icon"][type="image/x-icon"]')).toHaveAttribute(
    'href',
    '/favicon.ico',
  )
  const pngResponse = await request.get('/favicon.png')
  expect(pngResponse.status()).toBe(200)
  expect(pngResponse.headers()['content-type']).toContain('image/png')
  const png = await pngResponse.body()
  expect([...png.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10])
  expect(png.readUInt32BE(16)).toBe(96)
  expect(png.readUInt32BE(20)).toBe(96)
  const icoResponse = await request.get('/favicon.ico')
  expect(icoResponse.status()).toBe(200)
  expect(icoResponse.headers()['content-type']).toMatch(/image\/(?:x-icon|vnd\.microsoft\.icon)/)
  const ico = await icoResponse.body()
  expect(ico.readUInt16LE(0)).toBe(0)
  expect(ico.readUInt16LE(2)).toBe(1)
  expect(ico.readUInt16LE(4)).toBe(4)
  for (const [index, size] of [16, 32, 48, 96].entries()) {
    const entry = 6 + 16 * index
    expect(ico[entry]).toBe(size)
    expect(ico[entry + 1]).toBe(size)
    const length = ico.readUInt32LE(entry + 8)
    const offset = ico.readUInt32LE(entry + 12)
    expect(offset + length).toBeLessThanOrEqual(ico.length)
    if (size === 96) expect(ico.subarray(offset, offset + length)).toEqual(png)
  }
  const dimensions = await page.evaluate(async () => {
    const image = new Image()
    image.src = '/favicon.png'
    await image.decode()
    return [image.naturalWidth, image.naturalHeight]
  })
  expect(dimensions).toEqual([96, 96])
  const deployment = JSON.parse(readFileSync(new URL('../vercel.json', import.meta.url), 'utf8'))
  const spaRewrite = new RegExp(`^${deployment.rewrites[0].source}$`)
  for (const path of ['/favicon.svg', '/favicon.png', '/favicon.ico'])
    expect(spaRewrite.test(path)).toBe(false)
  expect(spaRewrite.test('/inbox')).toBe(true)
})

async function noOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  )
  const header = page.locator('header')
  expect(await header.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true)
  for (const label of await header.locator(':scope > span').all()) {
    if (await label.isVisible())
      expect(await label.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
        true,
      )
  }
}

async function checkLogo(link: Locator, dark: boolean) {
  await expect(link).toHaveAccessibleName('rakkio')
  await expect(link).toHaveAttribute('href', '/')
  await expect(link).toBeInViewport()
  const logo = link.getByRole('img', { name: 'rakkio', exact: true })
  await expect(logo).toHaveCount(1)
  await expect(logo).toBeVisible()
  await expect(logo).toHaveAttribute('viewBox', '0 0 97 40')
  await expect(logo.locator('title, text, [id]')).toHaveCount(0)
  // Use the rendered theme color (Tailwind uses OKLCH) rather than hard-coded RGB.
  expect(await logo.evaluate((svg) => getComputedStyle(svg).color)).toBe(
    await link.evaluate((anchor) => getComputedStyle(anchor).color),
  )
  expect(await logo.evaluate((svg) => getComputedStyle(svg.querySelector('path')!).fill)).toBe(
    await logo.evaluate((svg) => getComputedStyle(svg).color),
  )
  await expect(link.page().locator('html')).toHaveClass(dark ? /dark/ : /^(?!.*dark)/)
}

async function screenshot(page: Page, testInfo: TestInfo, name: string) {
  await page.screenshot({ path: testInfo.outputPath(`${name}.png`), animations: 'disabled' })
}

for (const colorScheme of ['light', 'dark'] as const) {
  test.describe(colorScheme, () => {
    test.use({ colorScheme })

    for (const path of ['/', '/privacy', '/terms']) {
      test(`public brand ${path}`, async ({ page }, testInfo) => {
        await mockWorkspace(page, false)
        await page.goto(path)
        if (path === '/')
          await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeVisible()
        else
          await expect(page.locator('h1')).toHaveText(
            path === '/privacy' ? 'Privacy Policy' : 'Terms of Service',
          )
        const home = page.getByRole('link', { name: 'rakkio', exact: true })
        await expect(home).toHaveCount(1)
        await checkLogo(home, colorScheme === 'dark')
        await expect(page).toHaveTitle(
          path === '/'
            ? 'rakkio'
            : `${path === '/privacy' ? 'Privacy Policy' : 'Terms of Service'} | rakkio`,
        )
        await noOverflow(page)
        await screenshot(page, testInfo, 'brand')
        if (testInfo.project.use.isMobile) {
          await page.setViewportSize({ width: 320, height: 900 })
          await noOverflow(page)
          await checkLogo(home, colorScheme === 'dark')
        }
        await home.focus()
        await page.keyboard.press('Enter')
        await expect(page).toHaveURL('/')
        await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeDisabled()
      })
    }

    for (const sidebarVisible of [true, false]) {
      test(`workspace sidebar ${sidebarVisible ? 'visible' : 'hidden'}`, async ({
        page,
      }, testInfo) => {
        await mockWorkspace(page, true, sidebarVisible)
        await page.goto('/notes')
        await expect(page.getByRole('heading', { name: 'Notes', exact: true })).toBeVisible()
        const sidebar = page.getByRole('complementary', { name: 'Primary navigation' })
        const header = page.locator('header')
        const home = page.getByRole('link', { name: 'rakkio', exact: true })
        await expect(home).toHaveCount(1)
        await checkLogo(home, colorScheme === 'dark')
        if (sidebarVisible && !testInfo.project.use.isMobile) {
          await expect(sidebar).toBeVisible()
          await expect(sidebar.getByRole('link', { name: 'rakkio', exact: true })).toBeVisible()
          await expect(header.getByRole('link', { name: 'rakkio', exact: true })).toHaveCount(0)
        } else {
          await expect(sidebar).toHaveCount(0)
          await expect(header.getByRole('link', { name: 'rakkio', exact: true })).toBeVisible()
        }
        await noOverflow(page)
        await screenshot(page, testInfo, 'brand')
        await home.click()
        await expect(page).toHaveURL('/')
        await expect(
          page.getByRole('heading', { name: 'Make today count.', exact: true }),
        ).toBeVisible()
        if (sidebarVisible && testInfo.project.use.isMobile) {
          const toggle = page.getByRole('button', { name: 'Open navigation' })
          // A translated-offscreen link must not remain keyboard-focusable.
          await page
            .locator('aside a')
            .first()
            .evaluate((anchor: HTMLElement) => anchor.focus())
          await expect(page.locator('aside a').first()).not.toBeFocused()
          await page.getByRole('link', { name: 'Skip to content' }).focus()
          await page.keyboard.press('Tab')
          await expect(toggle).toBeFocused()
          await page.keyboard.press('Enter')
          await expect(sidebar).toBeVisible()
          await expect(page.getByRole('button', { name: 'Close navigation' })).toBeFocused()
          await checkLogo(
            sidebar.getByRole('link', { name: 'rakkio', exact: true }),
            colorScheme === 'dark',
          )
          await screenshot(page, testInfo, 'brand-navigation-open')
          await page.keyboard.press('Escape')
          await expect(sidebar).toHaveCount(0)
          await expect(toggle).toBeFocused()
          await toggle.click()
          await sidebar.getByRole('link', { name: 'Notes', exact: true }).click()
          await expect(page).toHaveURL('/notes')
          await expect(toggle).toBeFocused()
          await toggle.click()
          await sidebar.getByRole('link', { name: 'rakkio', exact: true }).click()
          await expect(page).toHaveURL('/')
          await expect(toggle).toBeFocused()
          await expect(sidebar).toHaveCount(0)
          await toggle.click()
          await page.getByRole('button', { name: 'Close navigation' }).click()
          await expect(toggle).toBeFocused()
          await expect(sidebar).toHaveCount(0)
        }
        if (sidebarVisible && !testInfo.project.use.isMobile) {
          await sidebar.getByRole('link', { name: 'Notes', exact: true }).focus()
          await page.keyboard.press('Enter')
          await expect(page).toHaveURL('/notes')
        }
        const originalColor = await home.evaluate((link) => getComputedStyle(link).color)
        await page
          .getByRole('button', {
            name: colorScheme === 'dark' ? 'Use light theme' : 'Use dark theme',
          })
          .click()
        await checkLogo(home, colorScheme !== 'dark')
        expect(await home.evaluate((link) => getComputedStyle(link).color)).not.toBe(originalColor)
      })
    }

    test('favicon adapts on a browser light or dark surface', async ({ page }, testInfo) => {
      await page.goto('/favicon.svg')
      const svg = page.locator('svg')
      await expect(svg).toHaveAttribute('viewBox', '0 0 96 96')
      await expect(svg.locator('path')).toHaveCSS(
        'fill',
        colorScheme === 'dark' ? 'rgb(248, 250, 252)' : 'rgb(15, 23, 42)',
      )
      for (const size of [16, 32]) {
        await svg.evaluate(
          (element, { size, colorScheme }) => {
            element.style.width = `${size}px`
            element.style.height = `${size}px`
            element.style.backgroundColor = colorScheme === 'dark' ? '#020617' : '#fff'
          },
          { size, colorScheme },
        )
        await svg.screenshot({ path: testInfo.outputPath(`favicon-${size}px.png`) })
      }
    })
  })
}

for (const sidebarVisible of [true, false]) {
  test(`narrow header does not overflow with sidebar ${sidebarVisible}`, async ({
    page,
  }, testInfo) => {
    await mockWorkspace(page, true, sidebarVisible)
    await page.goto('/projects')
    await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible()
    for (const width of [640, 320, 375, 768, 1024, 1280]) {
      await page.setViewportSize({ width, height: 900 })
      const search = page.locator('header').getByRole('searchbox')
      if (width >= 640) {
        await search.focus()
        await expect(search).toHaveCSS('width', '256px')
      }
      await noOverflow(page)
      const brand = page.getByRole('link', { name: 'rakkio', exact: true })
      await expect(brand).toHaveCount(1)
      await expect(brand).toBeInViewport()
      if (width === 320 || width === 640)
        await screenshot(page, testInfo, `brand-header-${width}px`)
    }
  })
}
