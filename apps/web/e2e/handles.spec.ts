import { expect, test, type Page } from '@playwright/test'

async function settings(page: Page, initial: string | null = null) {
  let handle = initial
  let conflict = false
  const claims: unknown[] = []
  await page.route('**/api/v1/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '')
    let body: unknown = { items: [] }
    if (path === '/session')
      body = {
        user: {
          id: 'owner',
          display_name: 'Owner',
          email: 'owner@example.test',
          avatar_url: null,
          timezone: 'UTC',
        },
        csrf_token: 'mock-csrf',
      }
    else if (path === '/settings')
      body = {
        theme: 'system',
        automatic_daily_review: false,
        sidebar_visible: false,
        sync_conflict_policy: 'ask',
        version: 1,
      }
    else if (path.startsWith('/daily-plans')) body = { focus_tasks: [] }
    else if (path === '/profile/handle') {
      if (route.request().method() === 'POST') {
        expect(route.request().headers()['x-csrf-token']).toBe('mock-csrf')
        const value = route.request().postDataJSON()
        expect(value.expected_user_id).toBe('owner')
        claims.push(value)
        if (conflict) {
          await route.fulfill({
            status: 409,
            json: {
              error: { code: 'conflict', message: 'That handle is unavailable. Choose another.' },
            },
          })
          return
        }
        handle = value.handle
      }
      body = { handle }
    } else if (path === '/integrations/discord')
      body = {
        configured: false,
        connection: null,
        consent_version: '2026-11-03',
        install_url: null,
      }
    else if (path === '/integrations/google')
      body = { connected: false, calendar_authorized: false }
    else if (path === '/integrations/google/tasks')
      body = { authorized: false, enabled: false, version: 0 }
    else if (path.startsWith('/integrations/google/tasks/')) body = []
    else if (path === '/files/usage')
      body = { used_bytes: 0, max_user_storage_bytes: 104857600, max_file_size_bytes: 26214400 }
    await route.fulfill({ json: body })
  })
  return {
    claims,
    setConflict(value: boolean) {
      conflict = value
    },
  }
}

test('choose and acknowledge a permanent public handle without changing account identity', async ({
  page,
}, info) => {
  const mobile = !!info.project.use.isMobile
  await page.setViewportSize({ width: mobile ? 320 : 1280, height: 1000 })
  await page.emulateMedia({ colorScheme: mobile ? 'dark' : 'light' })
  const fixture = await settings(page)
  await page.goto('/settings')
  const panel = page.getByRole('region', { name: 'Your @handle', exact: true })
  const claim = panel.getByRole('button', { name: 'Claim handle', exact: true })
  await expect(claim).toBeDisabled()
  await panel.getByRole('textbox', { name: 'Handle', exact: true }).fill('@DaNiEl_18')
  await expect(panel.getByText('You are choosing', { exact: false })).toContainText('@daniel_18')
  await expect(claim).toBeDisabled()
  await panel.scrollIntoViewIfNeeded()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await panel.screenshot({ path: info.outputPath('handle-claim.png') })
  await panel.getByRole('checkbox').check()
  await claim.click()
  await expect(panel.getByRole('status')).toContainText('Your handle has been claimed')
  await expect(panel.getByText('@daniel_18', { exact: true })).toBeVisible()
  await expect(panel.getByRole('form')).toHaveCount(0)
  expect(fixture.claims).toEqual([{ handle: 'daniel_18', expected_user_id: 'owner' }])
  await panel.getByRole('button', { name: 'Refresh handle' }).click()
  await expect(panel.getByText('@daniel_18', { exact: true })).toBeVisible()
  expect(fixture.claims).toHaveLength(1)
})

test('invalid or changed choices do not reuse acknowledgement and conflicts require refresh', async ({
  page,
}) => {
  const fixture = await settings(page)
  fixture.setConflict(true)
  await page.goto('/settings')
  const panel = page.getByRole('region', { name: 'Your @handle', exact: true })
  const input = panel.getByRole('textbox', { name: 'Handle', exact: true })
  const claim = panel.getByRole('button', { name: 'Claim handle', exact: true })
  await input.fill('alice')
  await panel.getByRole('checkbox').check()
  await input.fill('Ａlice')
  await expect(panel.getByRole('checkbox')).not.toBeChecked()
  await expect(claim).toBeDisabled()
  await input.fill('alice')
  await panel.getByRole('checkbox').check()
  await claim.click()
  await expect(panel.getByRole('alert')).toContainText('Refresh')
  await expect(claim).toBeDisabled()
  expect(fixture.claims).toHaveLength(1)
  fixture.setConflict(false)
  await panel.getByRole('button', { name: 'Refresh handle' }).click()
  await expect(panel.getByRole('checkbox')).not.toBeChecked()
  await input.fill('available')
  await panel.getByRole('checkbox').check()
  await claim.click()
  await expect(panel.getByText('@available', { exact: true })).toBeVisible()
  expect(fixture.claims).toEqual([
    { handle: 'alice', expected_user_id: 'owner' },
    { handle: 'available', expected_user_id: 'owner' },
  ])
})

test('existing handle is read-only, with no account directory or rename controls', async ({
  page,
}) => {
  const fixture = await settings(page, 'classmate')
  await page.goto('/settings')
  const panel = page.getByRole('region', { name: 'Your @handle', exact: true })
  await expect(panel.getByText('@classmate', { exact: true })).toBeVisible()
  await expect(panel.getByRole('textbox')).toHaveCount(0)
  await expect(panel.getByRole('form')).toHaveCount(0)
  expect(fixture.claims).toHaveLength(0)
})
