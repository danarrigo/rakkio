import { randomUUID } from 'node:crypto'
import { expect, test, type Page } from '@playwright/test'
import type { CalendarMoveUndo, DailyPlan, MoveCalendarItemRequest } from '../src/api/types'

const now = '2026-09-01T09:00:00Z'

async function mockWorkspace(page: Page, initialEvents: Record<string, unknown>[] = []) {
  const tasks: Record<string, unknown>[] = []
  const events: Record<string, unknown>[] = [...initialEvents]
  const receipts: CalendarMoveUndo[] = []
  await page.route('**/api/v1/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '')
    let body: unknown = path === '/saved-task-views' ? [] : { items: [] }
    if (path === '/session')
      body = {
        csrf_token: 'mock-csrf',
        user: { id: 'user-1', email: 'me@example.test', display_name: 'Planner' },
      }
    else if (path === '/settings')
      body = {
        theme: 'system',
        automatic_daily_review: false,
        sidebar_visible: true,
        sync_conflict_policy: 'ask',
        version: 1,
      }
    else if (path.startsWith('/daily-plans/'))
      body = { date: path.slice('/daily-plans/'.length), focus_tasks: [] } satisfies DailyPlan
    else if (path === '/calendars')
      body = {
        items: [
          {
            id: 'calendar-1',
            name: 'Personal',
            source: 'native',
            color: '#64748b',
            selected: true,
            is_default: true,
            external_id: null,
            version: 1,
          },
        ],
      }
    else if (path === '/projects')
      body = {
        items: [{ id: 'project-1', name: 'Work', status: 'active', color: '#64748b', version: 1 }],
      }
    else if (path === '/calendar-move-undos' && route.request().method() === 'GET')
      body = {
        items: receipts
          .filter((receipt) => Date.parse(receipt.expires_at) > Date.now())
          .toReversed(),
      }
    else if (/^\/events\/[^/]+\/move$/.test(path) && route.request().method() === 'POST') {
      const event = events.find((event) => event.id === path.split('/')[2])
      if (!event) {
        await route.fulfill({ status: 404, json: { error: { code: 'not_found' } } })
        return
      }
      const input: MoveCalendarItemRequest = route.request().postDataJSON()
      if (input.expected_version !== event.version) {
        await route.fulfill({
          status: 409,
          json: { error: { code: 'conflict', message: 'Calendar item changed.' } },
        })
        return
      }
      Object.assign(event, {
        starts_at: input.starts_at,
        ends_at: input.ends_at,
        version: input.expected_version + 1,
      })
      const receipt: CalendarMoveUndo = {
        id: randomUUID(),
        event_id: String(event.id),
        task_id: null,
        expires_at: new Date(Date.now() + 60000).toISOString(),
      }
      receipts.push(receipt)
      body = receipt
    } else if (path.startsWith('/events/') && route.request().method() === 'PUT') {
      const index = events.findIndex((event) => event.id === path.split('/').at(-1))
      body = { ...events[index], ...route.request().postDataJSON(), version: 2 }
      events[index] = body as Record<string, unknown>
    } else if (path === '/tasks' || path === '/events') {
      const items = path === '/tasks' ? tasks : events
      if (route.request().method() === 'POST') {
        body = {
          ...route.request().postDataJSON(),
          id: `created-${items.length}`,
          version: 1,
          position: 1024,
          created_at: now,
          updated_at: now,
          completed_at: null,
          linked_task_id: null,
        }
        items.push(body as Record<string, unknown>)
      } else body = { items }
    }
    await route.fulfill({ json: body })
  })
  return { tasks, events }
}

async function openTask(page: Page) {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Make today count.', exact: true })).toBeVisible()
  await page.keyboard.press('n')
  return page.getByRole('dialog', { name: 'New task', exact: true })
}

async function openEvent(page: Page) {
  await page.goto('/calendar?date=2026-09-01&view=day')
  await page.getByRole('button', { name: 'New event', exact: true }).click()
  return page.getByRole('form', { name: 'New event', exact: true })
}

test('creates a title-only task by keyboard with optional fields empty', async ({
  page,
}, testInfo) => {
  const { tasks } = await mockWorkspace(page)
  const form = await openTask(page)
  await expect(form.getByLabel('Task title', { exact: true })).toBeFocused()
  await expect(form).not.toContainText(/\((?:optional|required[^)]*)\)/)
  await expect(form.getByRole('button', { name: 'Create task', exact: true })).toBeDisabled()
  await expect(form.getByText('Enter a task title to create it.')).toHaveCount(0)
  await expect(
    form.getByText('Only a title is required. A deadline does not reserve work time.'),
  ).toHaveCount(0)
  await expect(form.getByLabel('Project')).toHaveValue('')
  await expect(form.getByLabel('Deadline', { exact: false })).toHaveValue('')
  await form.getByLabel('Task title').fill('Write report')
  await page.screenshot({
    animations: 'disabled',
    path: testInfo.outputPath('after-minimal-task.png'),
  })
  await page.keyboard.press('Control+Enter')
  await expect(form).toBeHidden()
  expect(tasks[0]).toMatchObject({
    title: 'Write report',
    due_at: null,
    project_id: null,
    scheduled_start: null,
    scheduled_end: null,
    description: '',
    recurrence: 'none',
    labels: [],
    remind_at: null,
  })
})

test('keeps conditional task requirements visible and saves optional fields without scheduling', async ({
  page,
}) => {
  const { tasks } = await mockWorkspace(page)
  const form = await openTask(page)
  await form.getByLabel('Task title').fill('Write report')
  await form.getByRole('button', { name: 'Details' }).click()
  await form.getByLabel('Repeat').selectOption('weekly')
  await form.getByLabel('Description').fill('Bring notes')
  await form.getByLabel('Labels').fill('work, review')
  await form.getByLabel('Reminder').fill('2026-09-01T08:00')
  await form.getByRole('button', { name: 'Details' }).click()
  await expect(form.getByText(/Choose a deadline for repeating tasks/)).toBeVisible()
  await expect(
    form.getByRole('button', { name: 'Create task', exact: true }).filter({ visible: true }),
  ).toBeDisabled()
  await form.getByLabel('Project').selectOption('project-1')
  await form.getByLabel('Deadline', { exact: true }).fill('2026-09-02')
  await form.getByLabel('Deadline time').fill('15:00')
  await form
    .getByRole('button', { name: 'Create task', exact: true })
    .filter({ visible: true })
    .click()
  await expect(form).toBeHidden()
  expect(tasks[0]).toMatchObject({
    project_id: 'project-1',
    description: 'Bring notes',
    recurrence: 'weekly',
    labels: ['work', 'review'],
    scheduled_start: null,
    scheduled_end: null,
  })
  expect(tasks[0]?.due_at).toBe(
    await page.evaluate(() => new Date('2026-09-02T15:00').toISOString()),
  )
})

test('makes smart work blocks visible and supports keeping detected time as text', async ({
  page,
}, testInfo) => {
  const { tasks } = await mockWorkspace(page)
  const form = await openTask(page)
  await form.getByLabel('Task title').fill('Write report at 3pm')
  await expect(form.getByText(/Work time detected:/)).toBeVisible()
  await expect(form.getByText(/1-hour block/)).toBeVisible()
  await page.screenshot({
    animations: 'disabled',
    path: testInfo.outputPath('after-smart-work-time.png'),
  })
  await form.getByRole('button', { name: 'Keep at 3pm in task title' }).click()
  await expect(form.getByText(/Work time detected:/)).toBeHidden()
  await page.keyboard.press('Control+Enter')
  await expect(form).toBeHidden()
  expect(tasks[0]).toMatchObject({
    title: 'Write report at 3pm',
    due_at: null,
    scheduled_start: null,
    scheduled_end: null,
  })
})

test('requires calendar work time and explains an invalid range', async ({ page }, testInfo) => {
  const { tasks } = await mockWorkspace(page)
  await page.goto('/calendar?date=2026-09-01&view=day')
  await page.getByRole('button', { name: 'Create scheduled task at 09:00' }).click()
  const form = page.getByRole('form', { name: 'New scheduled task' })
  await expect(form.getByText(/This does not set a deadline/)).toBeVisible()
  await form.getByLabel('Title').fill('Write report')
  await form.getByLabel('Ends').fill('2026-09-01T08:00')
  await expect(form.getByText('End must be after start.', { exact: true }).first()).toBeVisible()
  await expect(form.getByRole('button', { name: 'Create task' })).toBeDisabled()
  await form.getByLabel('Ends').fill('2026-09-01T10:00')
  await page.screenshot({
    animations: 'disabled',
    path: testInfo.outputPath('after-scheduled-task.png'),
  })
  await form.getByRole('button', { name: 'Create task' }).click()
  await expect(form).toBeHidden()
  expect(tasks[0]).toMatchObject({ due_at: null, project_id: null, description: '' })
  expect(tasks[0]?.scheduled_start).toBe(
    await page.evaluate(() => new Date('2026-09-01T09:00').toISOString()),
  )
})

test('creates a timed event with optional defaults and actionable range validation', async ({
  page,
}, testInfo) => {
  const { events } = await mockWorkspace(page)
  const form = await openEvent(page)
  await expect(form.getByLabel('Title')).toBeFocused()
  await expect(form.getByRole('button', { name: 'Create event' })).toBeDisabled()
  await expect(form.getByText('Enter an event title.')).toBeVisible()
  await expect(form.getByLabel('Calendar')).toHaveValue('calendar-1')
  await form.getByLabel('Title').fill('Team meeting')
  await form.getByLabel('Ends').fill('2026-09-01T09:00')
  await expect(form.getByRole('button', { name: 'Create event' })).toBeDisabled()
  await expect(form.getByText('End must be after start.').first()).toBeVisible()
  await form.getByLabel('Ends').fill('2026-09-01T10:00')
  await page.screenshot({
    animations: 'disabled',
    path: testInfo.outputPath('after-timed-event.png'),
  })
  await form.getByRole('button', { name: 'Create event' }).click()
  await expect(form).toBeHidden()
  expect(events[0]).toMatchObject({
    all_day: false,
    calendar_id: 'calendar-1',
    location: '',
    attendees: [],
    description: '',
    recurrence: 'none',
    recurrence_until: null,
  })
})

test('validates optional event recurrence even when details close and drops a disabled repeat end', async ({
  page,
}) => {
  const { events } = await mockWorkspace(page)
  const form = await openEvent(page)
  await form.getByLabel('Title').fill('Team meeting')
  await form.locator('summary').click()
  await form.getByRole('combobox', { name: 'Repeat', exact: true }).selectOption('weekly')
  await form.getByLabel('Repeat until').fill('2026-08-01T09:00')
  await form.locator('summary').click()
  await expect(
    form.getByText('Repeat until must be after the event start, or leave it empty.').first(),
  ).toBeVisible()
  await expect(form.getByRole('button', { name: 'Create event' })).toBeDisabled()
  await form.locator('summary').click()
  await form.getByRole('combobox', { name: 'Repeat', exact: true }).selectOption('none')
  await form.getByLabel('Location').fill('Office')
  await form.getByLabel('Attendees').fill('alex@example.com, sam@example.com')
  await form.getByLabel('Description').fill('Planning')
  await form.getByRole('button', { name: 'Create event' }).click()
  await expect(form).toBeHidden()
  expect(events[0]).toMatchObject({
    location: 'Office',
    attendees: ['alex@example.com', 'sam@example.com'],
    description: 'Planning',
    recurrence: 'none',
    recurrence_until: null,
  })
})

for (const timezoneId of ['America/Los_Angeles', 'Asia/Tokyo']) {
  test.describe(`all-day dates in ${timezoneId}`, () => {
    test.use({ timezoneId })
    test('creates and edits date-only ranges with an inclusive last day', async ({
      page,
    }, testInfo) => {
      const { events } = await mockWorkspace(page)
      const form = await openEvent(page)
      await form.getByLabel('Title').fill('Away')
      await form.getByLabel('All day').check()
      await expect(form.getByLabel('Starts')).toHaveAttribute('type', 'date')
      await expect(form.getByLabel('Starts')).toHaveValue('2026-09-01')
      await expect(form.getByLabel('Last day')).toHaveValue('2026-09-01')
      await expect(form.locator('input[type=datetime-local]')).toHaveCount(0)
      await page.screenshot({
        animations: 'disabled',
        path: testInfo.outputPath('after-all-day-event.png'),
      })
      await form.getByRole('button', { name: 'Create event' }).click()
      await expect(form).toBeHidden()
      expect(events[0]).toMatchObject({
        starts_at: '2026-09-01T00:00:00.000Z',
        ends_at: '2026-09-02T00:00:00.000Z',
        all_day: true,
        timezone: 'UTC',
      })
      await page.getByRole('button', { name: 'Away', exact: true }).click()
      const edit = page.getByRole('form', { name: 'Edit event' })
      await expect(edit.getByLabel('Starts')).toHaveValue('2026-09-01')
      await expect(edit.getByLabel('Last day')).toHaveValue('2026-09-01')
      // A reopened date-only event has no prior timed draft, so use local defaults.
      await edit.getByLabel('All day').uncheck()
      await expect(edit.getByLabel('Starts')).toHaveValue('2026-09-01T09:00')
      await expect(edit.getByLabel('Ends')).toHaveValue('2026-09-01T10:00')
      await edit.getByLabel('All day').check()
      await edit.getByLabel('Last day').fill('2026-08-31')
      await expect(edit.getByRole('button', { name: 'Save event' })).toBeDisabled()
      await edit.getByLabel('Last day').fill('2026-09-03')
      await edit.getByRole('button', { name: 'Save event' }).click()
      await expect(edit).toBeHidden()
      expect(events[0]?.ends_at).toBe('2026-09-04T00:00:00.000Z')
      for (const mode of ['day', 'week', 'month', 'agenda']) {
        await page.goto(`/calendar?date=2026-09-03&view=${mode}`)
        await expect(page.getByText('Away', { exact: true }).first()).toBeVisible()
      }
      await page.goto('/calendar?date=2026-09-04&view=day')
      await expect(page.getByRole('button', { name: 'Away', exact: true })).toHaveCount(0)
      await page.goto('/calendar?date=2026-09-04&view=agenda')
      await expect(page.getByText('Away', { exact: true })).toHaveCount(0)
      await page.goto('/calendar?date=2026-09-04&view=month')
      await expect(
        page.getByRole('button', { name: /Friday, September 4, 2026, 0 events/ }),
      ).toBeVisible()
      await page.goto('/calendar?date=2026-09-04&view=week')
      const friday = page.getByRole('button', { name: 'Fri 4', exact: true }).locator('..')
      await expect(friday.getByText('Away', { exact: true })).toHaveCount(0)
      await expect(page.getByText('Away', { exact: true })).toHaveCount(3)
      // The existing week view uses native HTML5 drag, which is desktop-only.
      // Mobile date editing is covered above without claiming touch-drag support.
      if (!testInfo.project.use.isMobile) {
        await page
          .getByRole('button', { name: /^All day\s*Away$/ })
          .first()
          .dragTo(friday)
        await expect.poll(() => events[0]?.starts_at).toBe('2026-09-04T00:00:00.000Z')
        expect(events[0]?.ends_at).toBe('2026-09-07T00:00:00.000Z')
      }
    })
  })
}

for (const mode of ['new', 'edit']) {
  test(`preserves timed draft clocks across an all-day round trip in ${mode} event`, async ({
    page,
  }, testInfo) => {
    const { events } = await mockWorkspace(page)
    let form = await openEvent(page)
    await form.getByLabel('Title').fill('Team meeting')
    await form.getByLabel('Starts').fill('2026-09-01T15:00')
    await form.getByLabel('Ends').fill('2026-09-01T16:30')
    if (mode === 'edit') {
      await form.getByRole('button', { name: 'Create event' }).click()
      await expect(form).toBeHidden()
      await page.getByRole('button', { name: /^Edit Team meeting,/ }).click()
      form = page.getByRole('form', { name: 'Edit event' })
    }
    await form.getByLabel('All day').check()
    await form.getByLabel('All day').uncheck()
    await expect(form.getByLabel('Starts')).toHaveValue('2026-09-01T15:00')
    await expect(form.getByLabel('Ends')).toHaveValue('2026-09-01T16:30')
    await page.screenshot({
      animations: 'disabled',
      path: testInfo.outputPath('after-timing-round-trip.png'),
    })
    await form.getByRole('button', { name: mode === 'new' ? 'Create event' : 'Save event' }).click()
    await expect(form).toBeHidden()
    expect(events[0]?.starts_at).toBe(
      await page.evaluate(() => new Date('2026-09-01T15:00').toISOString()),
    )
    expect(events[0]?.ends_at).toBe(
      await page.evaluate(() => new Date('2026-09-01T16:30').toISOString()),
    )
  })
}

for (const { legacyEnd, changeDraft } of [
  { legacyEnd: '2026-09-01T10:00:34.000Z', changeDraft: false },
  { legacyEnd: '2026-09-01T09:00:34.000Z', changeDraft: false },
  { legacyEnd: '2026-09-01T10:00:34.000Z', changeDraft: true },
]) {
  test(`preserves legacy all-day timestamps ending ${legacyEnd} until explicitly converted${changeDraft ? ' from the current draft' : ''}`, async ({
    page,
  }, testInfo) => {
    const legacy = {
      id: 'legacy-1',
      calendar_id: 'calendar-1',
      title: 'Legacy',
      description: '',
      starts_at: '2026-09-01T09:00:12.000Z',
      ends_at: legacyEnd,
      all_day: true,
      timezone: 'Europe/London',
      location: '',
      attendees: [],
      recurrence: 'none',
      recurrence_until: null,
      version: 1,
    }
    const { events } = await mockWorkspace(page, [legacy])
    await page.goto('/calendar?date=2026-09-01&view=day')
    await page.getByRole('button', { name: 'Legacy', exact: true }).click()
    const form = page.getByRole('form', { name: 'Edit event' })
    await expect(form.getByText(/This older all-day event has saved times/)).toBeVisible()
    await form.getByLabel('Title').fill('Legacy renamed')
    await form.getByRole('button', { name: 'Save event' }).click()
    await expect(form).toBeHidden()
    expect(events[0]).toMatchObject({
      starts_at: legacy.starts_at,
      ends_at: legacy.ends_at,
      timezone: legacy.timezone,
    })
    await page.getByRole('button', { name: 'Legacy renamed', exact: true }).click()
    if (changeDraft) {
      await form.getByLabel('Starts').fill('2026-09-02T15:00')
      await form.getByLabel('Ends').fill('2026-09-03T16:30')
    }
    await form.getByRole('button', { name: 'Use all-day dates instead' }).click()
    await expect(form.getByLabel('Starts')).toHaveAttribute('type', 'date')
    await expect(form.getByLabel('Starts')).toHaveValue(changeDraft ? '2026-09-02' : '2026-09-01')
    await expect(form.getByLabel('Last day')).toHaveValue(changeDraft ? '2026-09-03' : '2026-09-01')
    if (changeDraft)
      await page.screenshot({
        animations: 'disabled',
        path: testInfo.outputPath('after-legacy-draft-conversion.png'),
      })
    await form.getByRole('button', { name: 'Save event' }).click()
    await expect(form).toBeHidden()
    expect(events[0]).toMatchObject({
      starts_at: changeDraft ? '2026-09-02T00:00:00.000Z' : '2026-09-01T00:00:00.000Z',
      ends_at: changeDraft ? '2026-09-04T00:00:00.000Z' : '2026-09-02T00:00:00.000Z',
      timezone: 'UTC',
    })
  })
}

test('keeps task draft and shows an inline retry message after an API error', async ({
  page,
}, testInfo) => {
  const { tasks } = await mockWorkspace(page)
  let fail = true
  await page.route('**/api/v1/tasks', async (route) => {
    if (route.request().method() === 'POST' && fail) {
      fail = false
      await route.fulfill({ status: 500, json: { error: 'failed' } })
    } else await route.fallback()
  })
  const form = await openTask(page)
  await form.getByLabel('Task title').fill('Write report')
  await page.keyboard.press('Control+Enter')
  await expect(form.getByRole('alert')).toHaveText(
    'Could not create task. Your draft is kept. Try again.',
  )
  await expect(form.getByLabel('Task title')).toHaveValue('Write report')
  await page.screenshot({
    animations: 'disabled',
    path: testInfo.outputPath('after-task-retry.png'),
  })
  await page.keyboard.press('Control+Enter')
  await expect(form).toBeHidden()
  expect(tasks).toHaveLength(1)
})

test('keeps event draft and shows a retry message after an API error', async ({ page }) => {
  await mockWorkspace(page)
  await page.route('**/api/v1/events', async (route) => {
    if (route.request().method() === 'POST')
      await route.fulfill({ status: 500, json: { error: 'failed' } })
    else await route.fallback()
  })
  const form = await openEvent(page)
  await form.getByLabel('Title').fill('Team meeting')
  await form.getByRole('button', { name: 'Create event' }).click()
  await expect(form.getByRole('alert')).toHaveText(
    'Could not save event. Your draft is kept. Try again.',
  )
  await expect(form.getByLabel('Title')).toHaveValue('Team meeting')
  await expect(form.getByRole('button', { name: 'Create event' })).toBeEnabled()
})

test('shows the missing required calendar and supports dark-mode keyboard disclosure', async ({
  page,
}, testInfo) => {
  await mockWorkspace(page)
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.route('**/api/v1/calendars', (route) => route.fulfill({ json: { items: [] } }))
  const form = await openEvent(page)
  await form.getByLabel('Title').fill('Team meeting')
  await expect(form.getByText(/Choose a calendar\. If none are available/)).toBeVisible()
  await expect(form.getByRole('button', { name: 'Create event' })).toBeDisabled()
  await form.locator('summary').focus()
  await page.keyboard.press('Enter')
  await expect(form.getByLabel('Location')).toBeVisible()
  await page.screenshot({
    animations: 'disabled',
    path: testInfo.outputPath('after-dark-event-details.png'),
  })
  await form.getByRole('button', { name: 'Create event' }).scrollIntoViewIfNeeded()
  await expect(form.getByRole('button', { name: 'Create event' })).toBeInViewport()
  await page.screenshot({
    animations: 'disabled',
    path: testInfo.outputPath('after-dark-event-feedback.png'),
  })
  await page.keyboard.press('Escape')
  await expect(form).toBeHidden()
})

test('saves explicit smart deadlines as timestamps and unqualified smart times as blocks', async ({
  page,
}) => {
  const { tasks } = await mockWorkspace(page)
  for (const prefix of ['due', 'by', '']) {
    const form = await openTask(page)
    await form.getByLabel('Task title').fill(`Write report ${prefix} tomorrow at 3pm`)
    if (prefix) {
      await expect(form.getByText(/Work time detected/)).toBeHidden()
      await expect(form.getByLabel('Deadline time')).toHaveValue('15:00')
    } else {
      await expect(form.getByText(/Work time detected/)).toBeVisible()
    }
    await page.keyboard.press('Control+Enter')
    await expect(form).toBeHidden()
    const task = tasks.at(-1)!
    if (prefix) {
      expect(task.scheduled_start).toBeNull()
      expect(task.scheduled_end).toBeNull()
      expect(await page.evaluate((due) => new Date(due).getHours(), task.due_at as string)).toBe(15)
    } else {
      expect(task.scheduled_start).not.toBeNull()
      expect(
        new Date(task.scheduled_end as string).getTime() -
          new Date(task.scheduled_start as string).getTime(),
      ).toBe(3_600_000)
    }
  }
})
