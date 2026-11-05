import { expect, test, type Page } from '@playwright/test'

async function fixture(
  page: Page,
  options: {
    member?: boolean
    enabled?: boolean
    conflict?: boolean
    shared?: boolean
    noticeVersion?: string
    revokeOnSave?: boolean
    description?: string
  } = {},
) {
  const actor = options.member ? 'student' : 'owner'
  const summary = {
    id: 'class',
    owner_id: 'owner',
    name: 'Algorithms class',
    timezone: 'Asia/Jakarta',
    approval_required: true,
    version: 1,
    deleted_at: null,
    undo_until: null,
    my_role: options.member ? 'member' : 'owner',
    my_membership_id: actor + '-membership',
    my_membership_version: 1,
    member_count: 3,
  }
  const initial = {
    id: 'assignment-1',
    group_id: 'class',
    title: 'Algorithms homework',
    description: options.description ?? 'Read the shared instructions.',
    due_on: '2026-11-12',
    due_time: '10:00:00',
    due_at: '2026-11-12T03:00:00Z',
    timezone: 'Asia/Jakarta',
    state: 'open',
    version: 1,
    my_progress: {
      completed_at: null as string | null,
      version: 0,
      seen_assignment_version: null as number | null,
    },
    changed_since_completion: false,
  }
  const rows = [
    initial,
    { ...initial, id: 'history-1', title: 'Archived worksheet', state: 'closed' },
  ]
  let prefs = {
    membership_id: summary.my_membership_id,
    version: 1,
    share_progress: false,
    notice_version: null as string | null,
    shared_at: null as string | null,
  }
  let undo: { id: string; title: string; version: number; undo_until: string } | null = null
  if (options.noticeVersion)
    prefs = {
      ...prefs,
      share_progress: true,
      notice_version: options.noticeVersion,
      shared_at: new Date().toISOString(),
    }
  let revoked = false
  let conflict = !!options.conflict
  const writes: { path: string; body: Record<string, unknown> }[] = []
  const reads: string[] = []
  await page.route('**/api/v1/**', async (route) => {
    const url = new URL(route.request().url()),
      path = url.pathname.replace('/api/v1', ''),
      method = route.request().method()
    let body: unknown = { items: [] }
    if (path === '/session')
      body = {
        user: {
          id: actor,
          display_name: actor,
          email: 'private@example.test',
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
    else if (path === '/groups/capability') body = { enabled: options.enabled ?? true }
    else if (path.startsWith('/groups')) {
      if (revoked && path.startsWith('/groups/class')) {
        await route.fulfill({
          status: 404,
          json: { error: { code: 'not_found', message: 'Group is no longer available.' } },
        })
        return
      }
      if (method === 'GET') reads.push(path)
      const input: Record<string, unknown> = method === 'GET' ? {} : route.request().postDataJSON()
      if (method !== 'GET') {
        expect(input.expected_user_id).toBe(actor)
        expect(route.request().headers()['x-csrf-token']).toBe('mock-csrf')
        writes.push({ path, body: input })
      }
      if (path === '/groups') body = [summary]
      else if (path === '/groups/invitations/inbox') body = { invitations: [], requests: [] }
      else if (path === '/groups/class')
        body = {
          group: summary,
          members: [],
          removed_members: [],
          invitations: [],
          join_requests: [],
          ownership_offer: null,
        }
      else if (path === '/groups/class/content-usage')
        body = { used_bytes: 2048, used_records: 8, max_bytes: 16777216, max_records: 20000 }
      else if (path === '/groups/class/preferences') {
        if (method === 'POST')
          prefs = {
            ...prefs,
            version: prefs.version + 1,
            share_progress: input.share_progress === true,
            notice_version: input.share_progress ? String(input.notice_version) : null,
            shared_at: input.share_progress ? new Date().toISOString() : null,
          }
        body = prefs
      } else if (path === '/groups/class/assignment-undos')
        body = { items: undo ? [undo] : [], next_cursor: null }
      else if (path === '/groups/class/assignments') {
        if (method === 'POST') {
          const row = {
            ...initial,
            id: 'assignment-new',
            title: String(input.title),
            description: String(input.description),
            due_on: input.due_on ? String(input.due_on) : '',
            due_time: input.due_time ? String(input.due_time) : '',
            my_progress: { completed_at: null, version: 0, seen_assignment_version: null },
          }
          rows.unshift(row)
          body = row
        } else
          body = {
            items: rows.filter(
              (row) =>
                row.state === (url.searchParams.get('state') || 'open') && row.id !== undo?.id,
            ),
            next_cursor: null,
          }
      } else if (path.endsWith('/progress-report'))
        body = {
          items: options.shared
            ? [
                {
                  user_id: 'alice',
                  handle: 'alice',
                  display_name: 'Alice',
                  completed_at: '2026-11-04T10:00:00Z',
                  seen_assignment_version: 1,
                },
              ]
            : [],
        }
      else if (path.startsWith('/groups/class/assignments/')) {
        const id = path.split('/')[4],
          row = rows.find((row) => row.id === id)!
        if (path.endsWith('/progress')) {
          expect(input.expected_membership_id).toBe(summary.my_membership_id)
          row.my_progress = {
            completed_at: input.completed ? '2026-11-04T11:00:00Z' : null,
            version: row.my_progress.version + 1,
            seen_assignment_version: input.completed ? row.version : null,
          }
          row.changed_since_completion = false
          body = row
        } else if (path.endsWith('/delete-with-undo')) {
          undo = {
            id: row.id,
            title: row.title,
            version: row.version + 1,
            undo_until: new Date(Date.now() + 60000).toISOString(),
          }
          body = undo
        } else if (path.endsWith('/restore')) {
          row.version = Number(input.expected_version) + 1
          undo = null
          body = row
        } else if (method === 'PUT') {
          if (options.revokeOnSave) {
            revoked = true
            await route.fulfill({
              status: 403,
              json: { error: { code: 'forbidden', message: 'Group access was removed.' } },
            })
            return
          }
          if (conflict) {
            conflict = false
            row.version++
            row.title = 'Another leader updated this'
            await route.fulfill({
              status: 409,
              json: {
                error: {
                  code: 'conflict',
                  message: 'The assignment changed. Refresh before continuing.',
                },
              },
            })
            return
          }
          expect(input.expected_version).toBe(row.version)
          row.title = String(input.title)
          row.description = String(input.description)
          row.state = String(input.state)
          row.version++
          body = row
        } else body = row
      }
    }
    await route.fulfill({ json: body })
  })
  await page.goto('/groups/class/assignments')
  return { writes, reads }
}

test('members complete only their own work and review closed history separately', async ({
  page,
}) => {
  const state = await fixture(page, { member: true })
  await expect(page.getByRole('heading', { name: 'Assignments', exact: true })).toBeVisible()
  await expect(page.getByText('Algorithms homework', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'New assignment', exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Mark complete', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Reopen my work', exact: true })).toBeVisible()
  expect(state.writes[0]?.body).toMatchObject({
    expected_assignment_version: 1,
    expected_progress_version: 0,
    completed: true,
  })
  await page.getByRole('button', { name: 'Closed history', exact: true }).click()
  await expect(page.getByText('Archived worksheet', { exact: true })).toBeVisible()
  await expect(page.getByText('Algorithms homework', { exact: true })).toHaveCount(0)
  expect(state.reads.some((path) => path.endsWith('/progress-report'))).toBe(false)
})

test('progress sharing requires a separate acknowledgement and can be stopped', async ({
  page,
}) => {
  const state = await fixture(page, { member: true })
  const consent = page.getByRole('checkbox', { name: /I agree to share my progress/ })
  await expect(consent).not.toBeChecked()
  await expect(
    page.getByRole('button', { name: 'Enable progress sharing', exact: true }),
  ).toBeDisabled()
  await consent.check()
  expect(state.writes).toHaveLength(0)
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(consent).not.toBeChecked()
  await consent.check()
  await page.getByRole('button', { name: 'Enable progress sharing', exact: true }).click()
  await expect(
    page.getByRole('button', { name: 'Stop sharing progress', exact: true }),
  ).toBeVisible()
  expect(state.writes[0]?.body).toMatchObject({
    share_progress: true,
    notice_version: '2026-11-04-progress',
    expected_membership_id: 'student-membership',
    expected_version: 1,
  })
  await page.getByRole('button', { name: 'Stop sharing progress', exact: true }).click()
  await expect(consent).not.toBeChecked()
  expect(state.writes[1]?.body.share_progress).toBe(false)
})

test('leaders publish group-owned assignments and delete only with confirmation and Undo', async ({
  page,
}) => {
  const state = await fixture(page)
  await page.getByRole('button', { name: 'New assignment', exact: true }).click()
  await page.getByLabel('Assignment title', { exact: true }).fill('Shared reading')
  await page.getByLabel('Description', { exact: true }).fill('Read chapter three.')
  await page.getByRole('button', { name: 'Publish assignment', exact: true }).click()
  const card = page
    .getByRole('article')
    .filter({ has: page.getByRole('heading', { name: 'Shared reading', exact: true }) })
  await expect(card).toBeVisible()
  expect(state.writes[0]?.body).toMatchObject({
    title: 'Shared reading',
    due_on: null,
    due_time: null,
  })
  page.once('dialog', (dialog) => dialog.dismiss())
  await card.getByRole('button', { name: 'Delete', exact: true }).click()
  expect(state.writes).toHaveLength(1)
  page.once('dialog', (dialog) => dialog.accept())
  await card.getByRole('button', { name: 'Delete', exact: true }).click()
  await expect(card).toHaveCount(0)
  await page.getByRole('button', { name: 'Undo deletion of Shared reading', exact: true }).click()
  await expect(card).toBeVisible()
})

test('conflicted edits preserve the draft and require explicit adoption of the current version', async ({
  page,
}) => {
  const state = await fixture(page, { conflict: true })
  await page.getByRole('button', { name: 'Edit', exact: true }).click()
  await page.getByLabel('Assignment title', { exact: true }).fill('My retained revision')
  await page.getByRole('button', { name: 'Save assignment', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Refresh')
  await expect(page.getByLabel('Assignment title', { exact: true })).toHaveValue(
    'My retained revision',
  )
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByText('Another leader updated this', { exact: true }).first()).toBeVisible()
  await expect(page.getByRole('button', { name: 'Save assignment', exact: true })).toBeDisabled()
  page.once('dialog', (dialog) => dialog.accept())
  await page.getByRole('button', { name: 'Use latest version with my draft', exact: true }).click()
  await page.getByRole('button', { name: 'Save assignment', exact: true }).click()
  await expect(
    page.getByRole('heading', { name: 'My retained revision', exact: true }),
  ).toBeVisible()
  expect(state.writes.map((write) => write.body.expected_version)).toEqual([1, 2])
})

test('leader reports show only explicitly shared progress and layout fits a narrow dark screen', async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 900 })
  await page.emulateMedia({ colorScheme: 'dark' })
  await fixture(page, { shared: true })
  await page.getByRole('button', { name: 'View shared progress', exact: true }).click()
  await expect(page.getByText('@alice', { exact: true })).toBeVisible()
  await expect(page.getByText(/Only members who opted in/)).toBeVisible()
  await page.getByRole('button', { name: 'New assignment', exact: true }).click()
  await expect(page.getByLabel('Assignment title', { exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320)
  await page.screenshot({
    path: test.info().outputPath('assignments-320-dark.png'),
    fullPage: true,
  })
})

test('unknown sharing notices are never automatically adopted', async ({ page }) => {
  const state = await fixture(page, { member: true, noticeVersion: 'unknown-future' })
  await expect(page.getByText(/accepted with a different notice version/)).toBeVisible()
  expect(state.writes).toHaveLength(0)
  await expect(page.getByRole('checkbox', { name: /I agree to share my progress/ })).toHaveCount(0)
  await page.getByRole('button', { name: 'Stop sharing progress', exact: true }).click()
  expect(state.writes[0]?.body).toMatchObject({ share_progress: false, notice_version: null })
})

test('lost access clears shared data but retains an unsaved draft for copying', async ({
  page,
}) => {
  await fixture(page, { revokeOnSave: true })
  await page.getByRole('button', { name: 'Edit', exact: true }).click()
  await page.getByLabel('Assignment title', { exact: true }).fill('My copyable draft')
  await page.getByRole('button', { name: 'Save assignment', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('removed')
  await expect(page.getByRole('article')).toHaveCount(0)
  await expect(page.getByLabel('Assignment title', { exact: true })).toHaveValue(
    'My copyable draft',
  )
  await expect(page.getByRole('button', { name: 'Save assignment', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByLabel('Assignment title', { exact: true })).toHaveValue(
    'My copyable draft',
  )
})

test('shared instructions cannot automatically fetch external media and long code stays contained', async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 900 })
  const external: string[] = []
  await page.route('https://tracking.invalid/**', async (route) => {
    external.push(route.request().url())
    await route.abort()
  })
  await fixture(page, {
    description:
      '```text\n' +
      'long-code'.repeat(80) +
      '\n```\n<img src="https://tracking.invalid/pixel"><video src="https://tracking.invalid/video"></video>',
  })
  await page.getByText('Instructions', { exact: true }).click()
  await expect(page.getByRole('article').locator('pre')).toBeVisible()
  await expect(page.getByRole('article').locator('img,video,svg,audio')).toHaveCount(0)
  expect(external).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320)
})

test('assignment editor and open work stay readable in a wide light layout', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.emulateMedia({ colorScheme: 'light' })
  await fixture(page)
  await page.getByRole('button', { name: 'New assignment', exact: true }).click()
  await page
    .getByLabel('Assignment title', { exact: true })
    .fill('Shared weekly algorithms assignment')
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(1280)
  await page.screenshot({
    path: test.info().outputPath('assignments-1280-light.png'),
    fullPage: true,
  })
})

test('closing moves shared work to history without completing the member', async ({ page }) => {
  const state = await fixture(page)
  await page.getByRole('button', { name: 'Edit', exact: true }).click()
  await page
    .getByRole('combobox', { name: 'Assignment lifecycle', exact: true })
    .selectOption('closed')
  await page.getByRole('button', { name: 'Save assignment', exact: true }).click()
  await expect(page.getByText('No open assignments.', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Closed history', exact: true }).click()
  const card = page
    .getByRole('article')
    .filter({ has: page.getByRole('heading', { name: 'Algorithms homework', exact: true }) })
  await expect(card.getByRole('button', { name: 'Mark complete', exact: true })).toBeVisible()
  expect(state.writes[0]?.body.state).toBe('closed')
})

test('navigation does not silently discard an assignment draft', async ({ page }) => {
  await fixture(page)
  await page.getByRole('button', { name: 'Edit', exact: true }).click()
  await page.getByLabel('Assignment title', { exact: true }).fill('Keep this draft')
  page.once('dialog', (dialog) => dialog.dismiss())
  await page.getByRole('link', { name: 'Back to Groups', exact: true }).click()
  await expect(page.getByLabel('Assignment title', { exact: true })).toHaveValue('Keep this draft')
  page.once('dialog', (dialog) => dialog.accept())
  await page.getByRole('link', { name: 'Back to Groups', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Groups', exact: true })).toBeVisible()
})

test('disabled groups do not fetch assignment data or offer mutations', async ({ page }) => {
  const state = await fixture(page, { enabled: false })
  await expect(
    page.getByText('Groups are not available on this server.', { exact: true }),
  ).toBeVisible()
  expect(state.reads.filter((path) => path.includes('assignments'))).toEqual([])
  expect(state.writes).toEqual([])
})
