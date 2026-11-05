import { randomUUID } from 'node:crypto'
import { expect, test } from '@playwright/test'

// Real disposable CI API/PostgreSQL only. No intercepted responses, providers,
// operator accounts or locally built backend. Hosted Groups remains disabled.
test('real backend: shared assignments preserve independent progress through edits, closing and Undo', async ({
  page,
  browser,
  baseURL,
}) => {
  test.skip(!process.env.CI, 'Backend/container execution is remote CI only.')
  test.setTimeout(90_000)
  expect(new URL(baseURL!).hostname).toBe('127.0.0.1')
  const studentContext = await browser.newContext({ baseURL })
  const student = await studentContext.newPage()
  let groupId: string | undefined
  let actor = ''
  let headers: Record<string, string> = {}
  try {
    await page.goto('/groups')
    await expect(page.getByRole('heading', { name: 'Groups', exact: true })).toBeVisible()
    await student.goto('/groups')
    await expect(student.getByRole('heading', { name: 'Groups', exact: true })).toBeVisible()
    const ownerSessionResponse = await page.request.get('/api/v1/session')
    const studentSessionResponse = await student.request.get('/api/v1/session')
    expect(ownerSessionResponse.status()).toBe(200)
    expect(studentSessionResponse.status()).toBe(200)
    const ownerSession = await ownerSessionResponse.json(),
      studentSession = await studentSessionResponse.json()
    actor = ownerSession.user.id
    headers = { 'x-csrf-token': ownerSession.csrf_token }
    expect(studentSession.user.id).not.toBe(actor)
    const studentHeaders = { 'x-csrf-token': studentSession.csrf_token }
    const suffix = randomUUID().replaceAll('-', '').slice(0, 12)
    const studentHandle = `ci_student_${suffix}`
    for (const [request, user, csrf, handle] of [
      [page.request, actor, headers, `ci_owner_${suffix}`],
      [student.request, studentSession.user.id, studentHeaders, studentHandle],
    ] as const) {
      const response = await request.post('/api/v1/profile/handle', {
        headers: csrf,
        data: { expected_user_id: user, handle },
      })
      expect(response.status(), await response.text()).toBe(200)
    }
    const created = await page.request.post('/api/v1/groups', {
      headers,
      data: {
        expected_user_id: actor,
        name: `CI algorithms ${suffix}`,
        timezone: 'Asia/Jakarta',
        roster_notice_version: '2026-11-04',
        confirm_roster_visibility: true,
      },
    })
    expect(created.status(), await created.text()).toBe(200)
    const group = await created.json()
    groupId = group.id
    const invited = await page.request.post(`/api/v1/groups/${groupId}/invitations`, {
      headers,
      data: {
        expected_user_id: actor,
        expected_version: group.version,
        kind: 'direct',
        handle: studentHandle,
        max_uses: 1,
      },
    })
    expect(invited.status(), await invited.text()).toBe(200)
    const invitation = await invited.json()
    const joined = await student.request.post('/api/v1/groups/join', {
      headers: studentHeaders,
      data: {
        expected_user_id: studentSession.user.id,
        invitation_id: invitation.invitation.id,
        roster_notice_version: '2026-11-04',
        confirm_roster_visibility: true,
      },
    })
    expect(joined.status(), await joined.text()).toBe(200)
    await page.reload()
    await page
      .getByRole('button')
      .filter({ hasText: `CI algorithms ${suffix}` })
      .click()
    await page.getByRole('link', { name: 'View assignments', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Assignments', exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'New assignment', exact: true }).click()
    const title = `Shared algorithms ${suffix}`
    await page.getByLabel('Assignment title', { exact: true }).fill(title)
    await page
      .getByLabel('Description', { exact: true })
      .fill('One canonical definition. Completion belongs to each member.')
    await page.getByLabel('Deadline date', { exact: true }).fill('2027-01-12')
    await page.getByRole('button', { name: 'Publish assignment', exact: true }).click()
    await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible()
    await student.goto(`/groups/${groupId}/assignments`)
    await expect(student.getByRole('heading', { name: title, exact: true })).toBeVisible()
    await student.getByRole('button', { name: 'Mark complete', exact: true }).click()
    await expect(student.getByRole('button', { name: 'Reopen my work', exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Refresh', exact: true }).click()
    await page.getByRole('button', { name: 'View shared progress', exact: true }).click()
    await expect(
      page.getByText('No members are sharing progress for this assignment.', { exact: true }),
    ).toBeVisible()
    await expect(page.getByRole('button', { name: 'Mark complete', exact: true })).toBeVisible()
    await student.getByRole('checkbox', { name: /I agree to share my progress/ }).check()
    await student.getByRole('button', { name: 'Enable progress sharing', exact: true }).click()
    await expect(
      student.getByRole('button', { name: 'Stop sharing progress', exact: true }),
    ).toBeVisible()
    await page.getByRole('button', { name: 'View shared progress', exact: true }).click()
    await expect(page.getByText(`@${studentHandle}`, { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Edit', exact: true }).click()
    await page.getByLabel('Assignment title', { exact: true }).fill(`${title} revised`)
    await page.getByRole('button', { name: 'Save assignment', exact: true }).click()
    await expect(page.getByRole('heading', { name: `${title} revised`, exact: true })).toBeVisible()
    await student.reload()
    await expect(
      student.getByRole('heading', { name: `${title} revised`, exact: true }),
    ).toBeVisible()
    await expect(student.getByRole('button', { name: 'Reopen my work', exact: true })).toBeVisible()
    await expect(student.getByText(/The assignment changed after your completion/)).toBeVisible()
    await student.getByRole('button', { name: 'Stop sharing progress', exact: true }).click()
    await expect(
      student.getByRole('checkbox', { name: /I agree to share my progress/ }),
    ).not.toBeChecked()
    await page.getByRole('button', { name: 'View shared progress', exact: true }).click()
    await expect(
      page.getByText('No members are sharing progress for this assignment.', { exact: true }),
    ).toBeVisible()
    await page.getByRole('button', { name: 'Edit', exact: true }).click()
    await page
      .getByRole('combobox', { name: 'Assignment lifecycle', exact: true })
      .selectOption('closed')
    await page.getByRole('button', { name: 'Save assignment', exact: true }).click()
    await expect(page.getByText('No open assignments.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Closed history', exact: true }).click()
    await expect(page.getByRole('heading', { name: `${title} revised`, exact: true })).toBeVisible()
    page.once('dialog', (dialog) => dialog.accept())
    await page.getByRole('button', { name: 'Delete', exact: true }).click()
    await page
      .getByRole('button', { name: `Undo deletion of ${title} revised`, exact: true })
      .click()
    await expect(page.getByRole('heading', { name: `${title} revised`, exact: true })).toBeVisible()
    await student.reload()
    await student.getByRole('button', { name: 'Closed history', exact: true }).click()
    await expect(
      student.getByRole('heading', { name: `${title} revised`, exact: true }),
    ).toBeVisible()
    await expect(student.getByRole('button', { name: 'Reopen my work', exact: true })).toBeVisible()
    for (const request of [page.request, student.request]) {
      const response = await request.get('/api/v1/tasks?limit=100')
      expect(response.status()).toBe(200)
      const tasks = await response.json()
      expect(tasks.items.some((row: { title: string }) => row.title.includes(suffix))).toBe(false)
    }
  } finally {
    if (groupId) {
      const response = await page.request.get(`/api/v1/groups/${groupId}`)
      if (response.ok()) {
        const current = await response.json()
        const deleted = await page.request.post(`/api/v1/groups/${groupId}/actions`, {
          headers,
          data: {
            expected_user_id: actor,
            expected_version: current.group.version,
            action: { kind: 'delete', confirmation: 'DELETE GROUP' },
          },
        })
        expect(deleted.status(), await deleted.text()).toBe(200)
      }
    }
    await studentContext.close()
  }
})
