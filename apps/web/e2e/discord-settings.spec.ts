import { expect, test, type Page } from '@playwright/test'
import type { DiscordStatus } from '../src/api/types'

async function settings(page: Page, linked = false, legacy = false) {
  let current: DiscordStatus = {
    configured: true,
    consent_version: '2026-11-03',
    install_url:
      'https://discord.com/oauth2/authorize?client_id=123456789012345678&scope=applications.commands&integration_type=1',
    connection: linked
      ? {
          id: '0198b80d-0000-7000-8000-000000000001',
          username: 'daniel',
          timezone: 'Asia/Jakarta',
          connected_at: '2026-10-31T00:00:00Z',
          consent_version: legacy ? '2026-10-31' : '2026-11-03',
        }
      : null,
  }
  let unavailable = false
  const writes: { path: string; body: unknown }[] = []
  await page.route('**/api/v1/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '')
    let body: unknown = { items: [] }
    if (path === '/session')
      body = {
        user: {
          id: 'owner',
          display_name: 'Owner',
          email: 'owner@example.test',
          timezone: 'UTC',
          avatar_url: null,
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
    else if (path === '/integrations/google')
      body = { connected: false, calendar_authorized: false }
    else if (path === '/integrations/google/tasks')
      body = { authorized: false, enabled: false, version: 0 }
    else if (path.startsWith('/integrations/google/tasks/')) body = []
    else if (path === '/files/usage')
      body = { used_bytes: 0, max_user_storage_bytes: 104857600, max_file_size_bytes: 26214400 }
    else if (path === '/integrations/discord') {
      if (unavailable) {
        await route.fulfill({
          status: 503,
          json: { error: { code: 'unavailable', message: 'Discord status unavailable.' } },
        })
        return
      }
      body = current
    } else if (path === '/integrations/discord/connect') {
      expect(route.request().headers()['x-csrf-token']).toBe('mock-csrf')
      writes.push({ path, body: route.request().postDataJSON() })
      body = {
        authorization_url:
          'https://discord.com/oauth2/authorize?client_id=123456789012345678&scope=identify&state=mock-browser-bound-state',
      }
    } else if (path === '/integrations/discord/consent') {
      expect(route.request().headers()['x-csrf-token']).toBe('mock-csrf')
      expect(route.request().postDataJSON()).toEqual({
        expected_id: current.connection?.id,
        consent_version: '2026-11-03',
        share_calendar_events: true,
      })
      writes.push({ path, body: route.request().postDataJSON() })
      current = {
        ...current,
        connection: current.connection
          ? { ...current.connection, consent_version: '2026-11-03' }
          : null,
      }
      body = current.connection
    } else if (path === '/integrations/discord/disconnect') {
      expect(route.request().headers()['x-csrf-token']).toBe('mock-csrf')
      expect(route.request().postDataJSON()).toEqual({ expected_id: current.connection?.id })
      writes.push({ path, body: route.request().postDataJSON() })
      current = { ...current, connection: null }
      await route.fulfill({ status: 204 })
      return
    }
    await route.fulfill({ json: body })
  })
  return {
    writes,
    setUnavailable: (value: boolean) => {
      unavailable = value
    },
  }
}

test('Discord is separate opt-in and redirects only after explicit data-sharing consent', async ({
  page,
}) => {
  const fixture = await settings(page)
  await page.route('https://discord.com/oauth2/authorize**', (route) =>
    route.fulfill({ contentType: 'text/html', body: '<h1>Mock Discord authorization</h1>' }),
  )
  await page.goto('/settings')
  const panel = page.getByRole('region', { name: 'Discord', exact: true })
  const connect = panel.getByRole('button', { name: 'Connect Discord', exact: true })
  await expect(connect).toBeDisabled()
  await expect(panel.getByText('No chat history access', { exact: false })).toBeVisible()
  await panel.getByRole('textbox', { name: 'Discord command timezone' }).fill('Asia/Jakarta')
  await panel.getByRole('checkbox').check()
  await expect(connect).toBeEnabled()
  expect(fixture.writes).toHaveLength(0)
  await connect.click()
  await expect(page.getByRole('heading', { name: 'Mock Discord authorization' })).toBeVisible()
  expect(fixture.writes).toEqual([
    {
      path: '/integrations/discord/connect',
      body: {
        timezone: 'Asia/Jakarta',
        consent_version: '2026-11-03',
        share_calendar_events: true,
      },
    },
  ])
})

test('existing Discord links add calendar sharing only after review without reconnecting', async ({
  page,
}, info) => {
  const mobile = !!info.project.use.isMobile
  await page.setViewportSize({ width: mobile ? 320 : 1280, height: 1000 })
  await page.emulateMedia({ colorScheme: mobile ? 'dark' : 'light' })
  const fixture = await settings(page, true, true)
  await page.goto('/settings')
  const panel = page.getByRole('region', { name: 'Discord', exact: true })
  const save = panel.getByRole('button', { name: 'Update Discord sharing', exact: true })
  await expect(save).toBeDisabled()
  await expect(
    panel.getByText('Your current notice covers tasks only.', { exact: false }),
  ).toBeVisible()
  expect(fixture.writes).toHaveLength(0)
  await panel.scrollIntoViewIfNeeded()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await panel.screenshot({ path: info.outputPath('discord-agenda-consent.png') })
  await panel.getByRole('checkbox').check()
  await save.click()
  await expect(panel.getByRole('status')).toContainText('Calendar events are enabled')
  await expect(panel.getByRole('checkbox')).toHaveCount(0)
  await expect(panel.getByText('Connected as', { exact: false })).toBeVisible()
  expect(fixture.writes).toHaveLength(1)
  expect(fixture.writes[0]?.path).toBe('/integrations/discord/consent')
})

test('Discord disconnect is confirmed and preserves the rest of the workspace', async ({
  page,
}) => {
  const fixture = await settings(page, true)
  await page.goto('/settings')
  const panel = page.getByRole('region', { name: 'Discord', exact: true })
  await expect(panel.getByText('Connected as', { exact: false })).toBeVisible()
  await expect(panel.getByRole('link', { name: 'Install Discord app' })).toHaveAttribute(
    'href',
    /scope=applications.commands/,
  )
  page.once('dialog', (dialog) => dialog.dismiss())
  await panel.getByRole('button', { name: 'Disconnect Discord' }).click()
  expect(fixture.writes).toHaveLength(0)
  page.once('dialog', (dialog) => dialog.accept())
  await panel.getByRole('button', { name: 'Disconnect Discord' }).click()
  await expect(panel.getByRole('status')).toContainText('Your tasks are unchanged')
  await expect(panel.getByRole('button', { name: 'Connect Discord', exact: true })).toBeDisabled()
  await expect(page.getByRole('form', { name: 'Workspace settings' })).toBeVisible()
  expect(fixture.writes).toHaveLength(1)
})

test('Discord status failure is unknown and requires a successful refresh before connecting', async ({
  page,
}) => {
  const fixture = await settings(page)
  fixture.setUnavailable(true)
  await page.goto('/settings')
  const panel = page.getByRole('region', { name: 'Discord', exact: true })
  await expect(panel.getByRole('alert')).toContainText('Refresh status')
  await expect(panel.getByRole('button', { name: 'Connect Discord', exact: true })).toHaveCount(0)
  fixture.setUnavailable(false)
  await panel.getByRole('button', { name: 'Refresh Discord status' }).click()
  await expect(panel.getByRole('checkbox')).toBeVisible()
  await expect(panel.getByRole('alert')).toHaveCount(0)
  expect(fixture.writes).toHaveLength(0)
})

for (const colorScheme of ['light', 'dark'] as const) {
  for (const width of [320, 1280]) {
    test(`Discord settings remain readable at ${width}px ${colorScheme}`, async ({
      page,
    }, info) => {
      await page.setViewportSize({ width, height: 1000 })
      await page.emulateMedia({ colorScheme })
      await settings(page)
      await page.goto('/settings')
      const panel = page.getByRole('region', { name: 'Discord', exact: true })
      await expect(panel.getByRole('checkbox')).toBeVisible()
      await panel.scrollIntoViewIfNeeded()
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      )
      await page.screenshot({ path: info.outputPath(`discord-${width}-${colorScheme}.png`) })
    })
  }
}
