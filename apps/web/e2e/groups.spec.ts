import { expect, test, type Page } from '@playwright/test'

async function fixture(
  page: Page,
  options: { enabled?: boolean; member?: boolean; guest?: boolean; empty?: boolean } = {},
) {
  const enabled = options.enabled ?? true
  let signedIn = !options.guest
  let exists = !options.empty
  const actor = options.member ? 'student' : 'owner'
  const role = options.member ? 'member' : 'owner'
  let row = {
    id: 'class',
    owner_id: 'owner',
    name: 'College class',
    timezone: 'Asia/Jakarta',
    approval_required: true,
    version: 1,
    deleted_at: null,
    undo_until: null,
  }
  const members = [
    {
      id: 'owner-membership',
      user_id: 'owner',
      handle: 'lecturer',
      display_name: 'Lecturer',
      role: 'owner',
      state: 'active',
      version: 1,
    },
    {
      id: 'student-membership',
      user_id: 'student',
      handle: 'student',
      display_name: 'Student',
      role: 'member',
      state: 'active',
      version: 1,
    },
  ]
  const invitations: {
    id: string
    group_id: string
    group_name: string
    kind: string
    recipient_handle: string | null
    expires_at: string
    max_uses: number
    uses: number
    version: number
  }[] = []
  const requests: {
    id: string
    group_id: string
    group_name: string
    state: string
    version: number
  }[] = []
  const writes: { path: string; body: Record<string, unknown> }[] = []
  let dataReads = 0
  const summary = () => ({
    ...row,
    my_role: role,
    my_membership_id: actor === 'owner' ? 'owner-membership' : 'student-membership',
    my_membership_version: 1,
    member_count: members.length,
  })
  await page.route('**/api/v1/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '')
    const method = route.request().method()
    let body: unknown = { items: [] }
    if (!signedIn && ['/session', '/development/session'].includes(path)) {
      await route.fulfill({
        status: 401,
        json: { error: { code: 'authentication_required', message: 'Sign in required.' } },
      })
      return
    }
    if (path === '/account' && method === 'DELETE') {
      await route.fulfill({
        status: 409,
        json: {
          error: {
            code: 'conflict',
            message: 'Transfer ownership or delete your owned groups first.',
          },
        },
      })
      return
    }
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
    else if (path === '/groups/capability') body = { enabled }
    else if (path === '/profile/handle')
      body = { handle: actor === 'owner' ? 'lecturer' : 'student' }
    else if (path.startsWith('/groups')) {
      if (method === 'GET') dataReads++
      let input: Record<string, unknown> = {}
      if (method === 'POST') {
        expect(route.request().headers()['x-csrf-token']).toBe('mock-csrf')
        input = route.request().postDataJSON()
        if (path !== '/groups/join/preview') expect(input.expected_user_id).toBe(actor)
        writes.push({ path, body: input })
      }
      if (path === '/groups') {
        if (method === 'POST') {
          exists = true
          row = { ...row, name: String(input.name), timezone: String(input.timezone) }
          members.splice(1)
        }
        body = method === 'POST' ? summary() : exists ? [summary()] : []
      } else if (path === '/groups/invitations/inbox') body = { invitations: [], requests }
      else if (path === '/groups/class')
        body = {
          group: summary(),
          members,
          removed_members: [],
          invitations: options.member ? [] : invitations,
          join_requests: [],
          ownership_offer: null,
        }
      else if (path === '/groups/class/invitations') {
        const invitation = {
          id: 'invite-' + invitations.length,
          group_id: 'class',
          group_name: row.name,
          kind: String(input.kind),
          recipient_handle:
            input.kind === 'direct' ? String(input.handle).replace(/^@/, '').toLowerCase() : null,
          expires_at: '2030-01-01T00:00:00Z',
          max_uses: Number(input.max_uses),
          uses: 0,
          version: 1,
        }
        invitations.push(invitation)
        body = {
          invitation,
          credential:
            input.kind === 'direct'
              ? null
              : input.kind === 'code'
                ? 'ABCD2345EFGH'
                : 'a'.repeat(43),
        }
      } else if (path === '/groups/class/actions') {
        if (input.expected_version !== row.version) {
          await route.fulfill({
            status: 409,
            json: {
              error: { code: 'conflict', message: 'The group changed. Refresh before continuing.' },
            },
          })
          return
        }
        const action = input.action as {
          kind: string
          name?: string
          timezone?: string
          approval_required?: boolean
        }
        if (action.kind === 'update')
          row = {
            ...row,
            name: action.name!,
            timezone: action.timezone!,
            approval_required: action.approval_required!,
            version: row.version + 1,
          }
        body = { group: summary() }
      } else if (path === '/groups/join/preview')
        body = { group_id: 'other-class', group_name: 'Other class', approval_required: true }
      else if (path === '/groups/join') {
        requests.push({
          id: 'pending',
          group_id: 'other-class',
          group_name: 'Other class',
          state: 'pending',
          version: 1,
        })
        body = { status: 'pending', group_id: 'other-class' }
      }
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
    writes,
    reads: () => dataReads,
    signIn: () => {
      signedIn = true
    },
    change: () => {
      row = { ...row, name: 'Changed by another session', version: row.version + 1 }
    },
  }
}

async function selectClass(page: Page) {
  await page
    .getByRole('complementary', { name: 'Your groups' })
    .getByRole('button', { name: /College class/ })
    .click()
  await expect(page.getByRole('region', { name: 'College class', exact: true })).toBeVisible()
}

test('creating a group requires explicit roster sharing and preserves the chosen timezone', async ({
  page,
}) => {
  const state = await fixture(page, { empty: true })
  await page.goto('/groups')
  const form = page.getByRole('form', { name: 'Create group', exact: true })
  await form.getByRole('textbox', { name: 'Group name', exact: true }).fill('Computer Science 2026')
  await form.getByRole('textbox', { name: 'Group timezone', exact: true }).fill('Asia/Jakarta')
  await expect(form.getByRole('button', { name: 'Create group', exact: true })).toBeDisabled()
  await form.getByRole('checkbox').check()
  await form.getByRole('button', { name: 'Create group', exact: true }).click()
  await expect(
    page.getByRole('heading', { name: 'Computer Science 2026', exact: true }),
  ).toBeVisible()
  expect(state.writes[0]?.body).toEqual({
    expected_user_id: 'owner',
    name: 'Computer Science 2026',
    timezone: 'Asia/Jakarta',
    confirm_roster_visibility: true,
    roster_notice_version: '2026-11-04',
  })
})

test('server-disabled groups stay out of navigation and do not load group data', async ({
  page,
}) => {
  const state = await fixture(page, { enabled: false })
  await page.goto('/groups')
  await expect(
    page.getByText('Groups are not available on this installation yet.', { exact: false }),
  ).toBeVisible()
  await expect(page.getByRole('link', { name: 'Groups', exact: true })).toHaveCount(0)
  await expect(page.getByRole('form', { name: 'Create group' })).toHaveCount(0)
  expect(state.reads()).toBe(0)
})

test('leaders invite by handle, code and one-time link without enrolling recipients', async ({
  page,
}, info) => {
  const mobile = !!info.project.use.isMobile
  await page.setViewportSize({ width: mobile ? 320 : 1280, height: 1000 })
  await page.emulateMedia({ colorScheme: mobile ? 'dark' : 'light' })
  const state = await fixture(page)
  await page.goto('/groups')
  await selectClass(page)
  const form = page.getByRole('form', { name: 'Invite group members' })
  await form.getByRole('textbox', { name: 'Recipient @handle' }).fill('@new_student')
  await form.getByRole('button', { name: 'Send invitation' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Invitation sent' })).toBeVisible()
  await expect(page.getByText('owner · 2 members', { exact: false }).first()).toBeVisible()
  expect(state.writes[0]?.body).toEqual({
    expected_user_id: 'owner',
    expected_version: 1,
    kind: 'direct',
    handle: '@new_student',
    max_uses: 1,
  })
  await form.getByRole('combobox', { name: 'Invitation type' }).selectOption('code')
  await form.getByRole('button', { name: 'Create invitation' }).click()
  await expect(page.getByRole('textbox', { name: 'Copy now - only shown once' })).toHaveValue(
    'ABCD2345EFGH',
  )
  await form.getByRole('combobox', { name: 'Invitation type' }).selectOption('link')
  await form.getByRole('button', { name: 'Create invitation' }).click()
  await expect(page.getByRole('textbox', { name: 'Copy now - only shown once' })).toHaveValue(
    /\/join#invite=a+/,
  )
  expect(state.writes.filter((write) => write.path === '/groups/join')).toHaveLength(0)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.evaluate(() => window.scrollTo(0, 0))
  await page.screenshot({ path: info.outputPath('groups.png'), fullPage: true })
})

test('members cannot see management controls and joining requires roster acknowledgement', async ({
  page,
}) => {
  const state = await fixture(page, { member: true })
  await page.goto('/groups')
  await selectClass(page)
  await expect(page.getByRole('button', { name: 'Edit group', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Delete group', exact: true })).toHaveCount(0)
  await expect(page.getByRole('form', { name: 'Invite group members' })).toHaveCount(0)
  const panel = page.getByRole('region', { name: 'Join a group', exact: true })
  await panel.getByRole('textbox', { name: 'Join code or invitation link' }).fill('ABCD-2345-EFGH')
  await panel.getByRole('button', { name: 'Review invitation', exact: true }).click()
  await expect(panel.getByText('Joining as', { exact: false })).toContainText('@student')
  const join = panel.getByRole('button', { name: 'Request to join', exact: true })
  await expect(join).toBeDisabled()
  expect(state.writes.filter((write) => write.path === '/groups/join')).toHaveLength(0)
  await panel.getByRole('checkbox').check()
  await join.click()
  await expect(panel.getByRole('status')).toContainText('awaiting leader approval')
  await expect(page.getByText('Awaiting approval', { exact: true })).toBeVisible()
  expect(state.writes.find((write) => write.path === '/groups/join')?.body).toEqual({
    credential: 'ABCD2345EFGH',
    expected_user_id: 'student',
    confirm_roster_visibility: true,
    roster_notice_version: '2026-11-04',
  })
})

test('stale group edits preserve drafts and require explicit version review', async ({ page }) => {
  const state = await fixture(page)
  await page.goto('/groups')
  await selectClass(page)
  await page.getByRole('button', { name: 'Edit group', exact: true }).click()
  const form = page.getByRole('form', { name: 'Edit group' })
  await form.getByRole('textbox', { name: 'Group name', exact: true }).fill('My unsaved class name')
  state.change()
  await form.getByRole('button', { name: 'Save group', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Refresh')
  await expect(form.getByRole('textbox', { name: 'Group name', exact: true })).toHaveValue(
    'My unsaved class name',
  )
  await page.getByRole('button', { name: 'Refresh groups', exact: true }).click()
  await expect(form.getByText('Latest values:', { exact: false })).toContainText(
    'Changed by another session',
  )
  await expect(form.getByRole('button', { name: 'Save group', exact: true })).toBeDisabled()
  page.once('dialog', (dialog) => dialog.accept())
  await form.getByRole('button', { name: 'Review and apply draft to latest version' }).click()
  await form.getByRole('button', { name: 'Save group', exact: true }).click()
  await expect(
    page.getByRole('heading', { name: 'My unsaved class name', exact: true }),
  ).toBeVisible()
  expect(
    state.writes
      .filter((write) => write.path.endsWith('/actions'))
      .map((write) => write.body.expected_version),
  ).toEqual([1, 2])
})

test('owned-group account deletion conflicts are actionable without signing out', async ({
  page,
}) => {
  await fixture(page)
  await page.goto('/settings')
  page.once('dialog', (dialog) => dialog.accept('DELETE'))
  await page.getByRole('button', { name: 'Delete my account', exact: true }).click()
  await expect(page.getByRole('alert').filter({ hasText: 'Transfer ownership' })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
})

test('invitation fragments survive sign-in without becoming OAuth parameters or automatic joins', async ({
  page,
}) => {
  const state = await fixture(page, { guest: true })
  const credential = 'a'.repeat(43)
  await page.goto('/join#invite=' + credential)
  await expect(page.getByRole('heading', { name: 'Group invitation', exact: true })).toBeVisible()
  await expect(page.getByRole('link', { name: 'Sign in to rakkio', exact: true })).toBeVisible()
  expect(new URL(page.url()).hash).toBe('')
  expect(
    await page.evaluate(
      () => JSON.parse(sessionStorage.getItem('rakkio.pending-group-invite') || 'null')?.credential,
    ),
  ).toBe(credential)
  await page.getByRole('link', { name: 'Sign in to rakkio', exact: true }).click()
  expect(new URL(page.url()).search).toBe('')
  expect(state.writes).toHaveLength(0)
  state.signIn()
  await page.goto('/groups')
  await expect(page.getByRole('link', { name: 'Review invitation', exact: true })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Join code or invitation link' })).toHaveValue(
    credential,
  )
  expect(state.writes.filter((write) => write.path === '/groups/join')).toHaveLength(0)
})
