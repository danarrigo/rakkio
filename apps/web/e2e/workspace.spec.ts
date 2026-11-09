import { readFile } from 'node:fs/promises'
import { expect, test, type Page } from '@playwright/test'
import type { CalendarEvent, CalendarMoveUndo, MoveCalendarItemRequest } from '../src/api/types'

async function waitForEventRefresh(page: Page, eventId: string, version: number) {
  const response = await page.waitForResponse(async (response) => {
    if (
      new URL(response.url()).pathname !== '/api/v1/events' ||
      response.request().method() !== 'GET' ||
      response.status() !== 200
    )
      return false
    const body: { items: CalendarEvent[] } = await response.json()
    return body.items.some((event) => event.id === eventId && event.version === version)
  })
  const body: { items: CalendarEvent[] } = await response.json()
  return body.items.find((event) => event.id === eventId)!
}

async function waitForEventMove(page: Page, previous: CalendarEvent) {
  const [response, event] = await Promise.all([
    page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === `/api/v1/events/${previous.id}/move` &&
        response.request().method() === 'POST' &&
        response.status() === 200,
    ),
    waitForEventRefresh(page, previous.id, previous.version + 1),
  ])
  const receipt: CalendarMoveUndo = await response.json()
  const input: MoveCalendarItemRequest = response.request().postDataJSON()
  expect(receipt.event_id).toBe(previous.id)
  expect(input.expected_version).toBe(previous.version)
  expect(Date.parse(event.starts_at)).toBe(Date.parse(input.starts_at))
  expect(Date.parse(event.ends_at)).toBe(Date.parse(input.ends_at))
  await expect(
    page.getByRole('region', { name: 'Calendar move Undo' }).getByRole('button', {
      name: 'Undo',
      exact: true,
    }),
  ).toBeEnabled()
  return { receipt, event }
}

test('publishes a descriptive signed-out homepage and legal policies', async ({ page }) => {
  await page.route('**/api/v1/session', async (route) => {
    await route.fulfill({ status: 401, contentType: 'application/json', body: '{}' })
  })
  await page.route('**/api/v1/development/session', async (route) => {
    await route.fulfill({ status: 401, contentType: 'application/json', body: '{}' })
  })

  await page.goto('/')
  await expect(
    page.getByRole('heading', { name: 'Plan what matters without scattering your attention.' }),
  ).toBeVisible()
  const signIn = page.getByRole('button', { name: 'Sign in with Google' })
  await expect(signIn).toBeDisabled()
  await page.getByRole('checkbox', { name: /I confirm that I am at least 18/ }).check()
  await expect(signIn).toBeEnabled()
  await expect(page.getByRole('link', { name: 'Privacy Policy' })).toBeVisible()

  await page.goto('/?auth_error=capacity_reached')
  await expect(page.getByRole('alert')).toContainText('registrations are temporarily paused')

  await page.goto('/privacy')
  await expect(page).toHaveTitle('Privacy Policy | rakkio')
  await expect(page.getByRole('heading', { name: 'Privacy Policy', level: 1 })).toBeVisible()
  await expect(page.getByText('Google API Services User Data Policy')).toBeVisible()

  await page.goto('/terms')
  await expect(page).toHaveTitle('Terms of Service | rakkio')
  await expect(page.getByRole('heading', { name: 'Terms of Service', level: 1 })).toBeVisible()
  await expect(page.getByText('laws of the Republic of Indonesia')).toBeVisible()
})

test('does not render demo identity or unfinished feature placeholders', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Make today count.' })).toBeVisible()
  await expect(page.locator('body')).not.toContainText(
    /Soon|Calendar foundation comes next|rakkio Developer/i,
  )
})

test('opens the full calendar from Today and browses months', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Calendar', level: 2 })).toBeVisible()
  await page.getByRole('button', { name: 'View month' }).click()
  await expect(page).toHaveURL(/\/calendar\?date=\d{4}-\d{2}-\d{2}/)
  await expect(page.getByRole('heading', { name: 'Calendar', level: 1 })).toBeVisible()

  const month = page.locator('section').first().getByRole('heading', { level: 2 })
  const initialMonth = await month.textContent()
  await page.getByRole('button', { name: 'Next month' }).click()
  await expect(month).not.toHaveText(initialMonth ?? '')
})

test('opens a scheduled task directly from an empty calendar slot', async ({ page }) => {
  await page.goto('/calendar?view=day')
  await page.getByRole('button', { name: 'Create scheduled task at 09:00' }).click()

  const form = page.getByRole('form', { name: 'New scheduled task' })
  await expect(form).toBeVisible()
  await expect(form.getByLabel('Starts')).toHaveValue(/T09:00$/)
  await expect(form.getByLabel('Ends')).toHaveValue(/T10:00$/)
  await expect(page.getByRole('dialog', { name: 'Add to calendar' })).toHaveCount(0)

  await page.keyboard.press('Escape')
  await expect(form).toBeHidden()
})

test('navigates and opens actions without a mouse', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Make today count.', exact: true })).toBeVisible()

  await page.keyboard.press('g')
  await page.keyboard.press('c')
  await expect(page).toHaveURL(/\/calendar$/)

  await page.keyboard.press('e')
  const eventForm = page.getByRole('form', { name: 'New event' })
  await expect(eventForm).toBeVisible()
  await expect(eventForm.getByLabel('Title')).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(eventForm).toBeHidden()

  await page.keyboard.press('g')
  await page.keyboard.press('n')
  await expect(page).toHaveURL(/\/notes$/)

  await page.keyboard.press('Control+k')
  const palette = page.getByRole('dialog', { name: 'Command palette' })
  await expect(palette).toBeVisible()
  await palette.getByLabel('Search commands').fill('projects')
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(/\/projects$/)
  await expect(palette).toBeHidden()

  await page.keyboard.press('?')
  const shortcutHelp = page.getByRole('dialog', { name: 'Keyboard shortcuts' })
  await expect(shortcutHelp).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(shortcutHelp).toBeHidden()

  await page.keyboard.press('n')
  const taskDialog = page.getByRole('dialog', { name: 'New task' })
  await expect(taskDialog).toBeVisible()
  await expect(taskDialog.getByLabel('Task title')).toBeFocused()
  await page.keyboard.press('Escape')
})

test('hides the sidebar persistently and keeps Settings reachable', async ({ page, isMobile }) => {
  await page.goto('/settings')
  const settings = page.getByRole('form', { name: 'Workspace settings' })
  const sidebar = page.getByRole('complementary', { name: 'Primary navigation' })
  const toggle = settings.getByRole('checkbox', { name: 'Show sidebar' })
  await expect(toggle).toBeChecked()

  const hidden = page.waitForResponse(
    (response) =>
      response.url().endsWith('/api/v1/settings') &&
      response.request().method() === 'PUT' &&
      response.status() === 200,
  )
  await toggle.uncheck()
  await settings.getByRole('button', { name: 'Save settings' }).click()
  await hidden
  await expect(sidebar).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Open settings' })).toBeVisible()

  await page.reload()
  await expect(sidebar).toHaveCount(0)

  const restored = page.waitForResponse(
    (response) =>
      response.url().endsWith('/api/v1/settings') &&
      response.request().method() === 'PUT' &&
      response.status() === 200,
  )
  await settings.getByRole('checkbox', { name: 'Show sidebar' }).check()
  await settings.getByRole('button', { name: 'Save settings' }).click()
  await restored
  if (isMobile) {
    await expect(sidebar).toBeHidden()
    await page.getByRole('button', { name: 'Open navigation' }).click()
  }
  await expect(sidebar).toBeVisible()
})

test('keeps times visible on compact events and scheduled tasks', async ({ page }) => {
  const suffix = `${test.info().project.name}-${Date.now().toString().slice(-6)}`
  const eventName = `Compact event ${suffix}`
  const taskName = `Compact task ${suffix}`

  await page.goto('/calendar?view=day')
  await page.getByRole('button', { name: 'New event' }).click()
  const eventForm = page.getByRole('form', { name: 'New event' })
  const eventStart = await eventForm.getByLabel('Starts').inputValue()
  await eventForm.getByLabel('Title').fill(eventName)
  await eventForm.getByLabel('Ends').fill(eventStart.replace(/\d{2}:\d{2}$/, '09:15'))
  const created = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === '/api/v1/events' &&
      response.request().method() === 'POST' &&
      response.status() === 201,
  )
  await eventForm.getByRole('button', { name: 'Create event' }).click()
  const event: CalendarEvent = await (await created).json()

  const eventBlock = page.getByRole('button', { name: new RegExp(`Edit ${eventName}`) })
  await eventBlock.scrollIntoViewIfNeeded()
  const eventBox = await eventBlock.boundingBox()
  const eventTimeBox = await eventBlock.locator('time').boundingBox()
  expect(eventBox).not.toBeNull()
  expect(eventTimeBox).not.toBeNull()
  expect(eventBox!.height).toBeLessThanOrEqual(30)
  expect(eventTimeBox!.y).toBeGreaterThanOrEqual(eventBox!.y)
  expect(eventTimeBox!.y + eventTimeBox!.height).toBeLessThanOrEqual(eventBox!.y + eventBox!.height)

  const initialEventLabel = await eventBlock.getAttribute('aria-label')
  expect(initialEventLabel).not.toBeNull()
  const resized = waitForEventMove(page, event)
  await eventBlock.focus()
  await page.keyboard.press('Shift+ArrowDown')
  await resized
  await expect(eventBlock).not.toHaveAttribute('aria-label', initialEventLabel!)

  const thirtyMinuteBox = await eventBlock.boundingBox()
  expect(thirtyMinuteBox).not.toBeNull()
  expect(thirtyMinuteBox!.height).toBeGreaterThan(eventBox!.height)
  expect(Math.abs(thirtyMinuteBox!.height - eventBox!.height * 2)).toBeLessThanOrEqual(1)

  await eventBlock.click()
  const editForm = page.getByRole('form', { name: 'Edit event' })
  await editForm.getByRole('button', { name: 'Delete event' }).click()
  await editForm.getByRole('button', { name: 'Confirm delete' }).click()
  await expect(eventBlock).toHaveCount(0)

  await page.getByRole('button', { name: 'Create scheduled task at 10:00' }).click()
  const taskForm = page.getByRole('form', { name: 'New scheduled task' })
  const taskStart = await taskForm.getByLabel('Starts').inputValue()
  await taskForm.getByLabel('Title').fill(taskName)
  await taskForm.getByLabel('Ends').fill(taskStart.replace(/\d{2}:\d{2}$/, '10:15'))
  await taskForm.getByRole('button', { name: 'Create task' }).click()

  const taskBlock = page.getByRole('button', {
    name: new RegExp(`${taskName}, scheduled task`),
  })
  await taskBlock.scrollIntoViewIfNeeded()
  const taskBox = await taskBlock.boundingBox()
  const taskTimeBox = await taskBlock.locator('time').boundingBox()
  expect(taskBox).not.toBeNull()
  expect(taskTimeBox).not.toBeNull()
  expect(taskBox!.height).toBeLessThanOrEqual(30)
  expect(taskTimeBox!.y).toBeGreaterThanOrEqual(taskBox!.y)
  expect(taskTimeBox!.y + taskTimeBox!.height).toBeLessThanOrEqual(taskBox!.y + taskBox!.height)
})

test('creates an event and browses day, week, month, and agenda views', async ({ page }) => {
  const eventName = `Calendar event ${test.info().project.name}-${Date.now().toString().slice(-6)}`

  await page.goto('/calendar')
  await page.getByRole('button', { name: 'New event' }).click()
  const form = page.getByRole('form', { name: 'New event' })
  await form.getByLabel('Title').fill(eventName)
  const created = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === '/api/v1/events' &&
      response.request().method() === 'POST' &&
      response.status() === 201,
  )
  await form.getByRole('button', { name: 'Create event' }).click()
  const event: CalendarEvent = await (await created).json()
  await expect(page.getByText(eventName, { exact: true }).last()).toBeVisible()

  for (const mode of ['day', 'week', 'month', 'agenda']) {
    await page.getByRole('button', { name: mode, exact: true }).click()
    await expect(page).toHaveURL(new RegExp(`view=${mode}`))
    await expect(page.getByText(eventName, { exact: true }).last()).toBeVisible()
  }

  await page.getByRole('button', { name: 'day', exact: true }).click()
  const eventBlock = page.getByRole('button', { name: new RegExp(`Edit ${eventName}`) })
  await eventBlock.scrollIntoViewIfNeeded()
  const eventBox = await eventBlock.boundingBox()
  const initialEventLabel = await eventBlock.getAttribute('aria-label')
  expect(eventBox).not.toBeNull()
  expect(initialEventLabel).not.toBeNull()
  const moved = waitForEventMove(page, event)
  const moveStartX = eventBox!.x + eventBox!.width / 2
  const moveStartY = eventBox!.y + eventBox!.height / 2
  await page.mouse.move(moveStartX, moveStartY)
  await page.mouse.down()
  await page.mouse.move(moveStartX, moveStartY - 48, { steps: 5 })
  await page.mouse.up()
  const { event: movedEvent } = await moved
  await expect(eventBlock).not.toHaveAttribute('aria-label', initialEventLabel!)
  const movedEventLabel = await eventBlock.getAttribute('aria-label')
  expect(movedEventLabel).not.toBeNull()

  const bottomHandle = eventBlock.locator('[title="Drag bottom edge to resize"]')
  await bottomHandle.hover()
  const bottomBox = await bottomHandle.boundingBox()
  expect(bottomBox).not.toBeNull()
  const resized = waitForEventMove(page, movedEvent)
  await page.mouse.down()
  await expect(page.locator('body')).toHaveCSS('cursor', 'ns-resize')
  await page.mouse.move(
    bottomBox!.x + bottomBox!.width / 2,
    bottomBox!.y + bottomBox!.height / 2 + 48,
    { steps: 5 },
  )
  await page.mouse.up()
  const { event: resizedEvent } = await resized
  await expect(eventBlock).not.toHaveAttribute('aria-label', movedEventLabel!)
  const resizedEventLabel = await eventBlock.getAttribute('aria-label')
  expect(resizedEventLabel).not.toBeNull()

  const topHandle = eventBlock.locator('[title="Drag top edge to trim"]')
  await topHandle.hover()
  const topBox = await topHandle.boundingBox()
  expect(topBox).not.toBeNull()
  const trimmed = waitForEventMove(page, resizedEvent)
  await page.mouse.down()
  await page.mouse.move(topBox!.x + topBox!.width / 2, topBox!.y + topBox!.height / 2 + 24, {
    steps: 4,
  })
  await page.mouse.up()
  const { receipt, event: trimmedEvent } = await trimmed
  await expect(eventBlock).not.toHaveAttribute('aria-label', resizedEventLabel!)
  const trimmedEventLabel = await eventBlock.getAttribute('aria-label')
  expect(trimmedEventLabel).not.toBeNull()

  const recovered = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === '/api/v1/calendar-move-undos' &&
      response.request().method() === 'GET' &&
      response.status() === 200,
  )
  await page.reload()
  const activeReceipts: { items: CalendarMoveUndo[] } = await (await recovered).json()
  expect(activeReceipts.items).toContainEqual(receipt)
  await expect(eventBlock).toHaveAttribute('aria-label', trimmedEventLabel!)
  const feedback = page.getByRole('region', { name: 'Calendar move Undo' })
  const undo = feedback.getByRole('button', { name: 'Undo', exact: true })
  await expect(undo).toBeEnabled()
  const consumed = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === `/api/v1/calendar-move-undos/${receipt.id}/consume` &&
      response.request().method() === 'POST' &&
      response.status() === 204,
  )
  const restored = waitForEventRefresh(page, event.id, trimmedEvent.version + 1)
  await undo.click()
  await consumed
  const restoredEvent = await restored
  expect(restoredEvent).toMatchObject({
    id: event.id,
    starts_at: resizedEvent.starts_at,
    ends_at: resizedEvent.ends_at,
  })
  expect(restoredEvent.version).toBeGreaterThan(trimmedEvent.version)
  await expect(feedback).toContainText('Move undone.')
  await expect(eventBlock).toHaveAttribute('aria-label', resizedEventLabel!)
  await eventBlock.scrollIntoViewIfNeeded()

  const deleteBox = await eventBlock.boundingBox()
  expect(deleteBox).not.toBeNull()
  await page.mouse.move(deleteBox!.x + deleteBox!.width / 2, deleteBox!.y + 12)
  await page.mouse.down()
  await page.mouse.move(deleteBox!.x + deleteBox!.width / 2, deleteBox!.y + 24, { steps: 3 })
  const deleteZone = page.getByText('Drop here to delete', { exact: true })
  const deleteZoneBox = await deleteZone.boundingBox()
  expect(deleteZoneBox).not.toBeNull()
  const deleted = page.waitForResponse(
    (response) =>
      /\/api\/v1\/events\/[0-9a-f-]+/.test(response.url()) &&
      response.request().method() === 'DELETE' &&
      response.status() === 204,
  )
  await page.mouse.move(
    deleteZoneBox!.x + deleteZoneBox!.width / 2,
    deleteZoneBox!.y + deleteZoneBox!.height / 2,
    { steps: 8 },
  )
  await expect(page.getByText('Release to delete', { exact: true })).toBeVisible()
  await page.mouse.up()
  await deleted
  await expect(eventBlock).toHaveCount(0)
})

test('uploads, downloads, and deletes a private file', async ({ page }) => {
  const filename = `brief-${test.info().project.name}-${Date.now().toString().slice(-6)}.txt`
  await page.goto('/files')
  await page.locator('input[type="file"]').setInputFiles({
    name: filename,
    mimeType: 'text/plain',
    buffer: Buffer.from('private file contents'),
  })
  await expect(page.getByText(filename, { exact: true })).toBeVisible()
  const download = page.waitForEvent('download')
  await page.getByRole('link', { name: `Download ${filename}` }).click()
  const completedDownload = await download
  expect(completedDownload.suggestedFilename()).toBe(filename)
  expect(await completedDownload.failure()).toBeNull()
  const downloadedPath = await completedDownload.path()
  expect(downloadedPath).not.toBeNull()
  expect(await readFile(downloadedPath!, 'utf8')).toBe('private file contents')
  page.once('dialog', (dialog) => dialog.accept())
  await page.getByRole('button', { name: `Delete ${filename}` }).click()
  await expect(page.getByText(filename, { exact: true })).toHaveCount(0)
})

test('creates, safely renders, searches, and deletes a Markdown note', async ({ page }) => {
  const noteTitle = `Release note ${test.info().project.name}-${Date.now().toString().slice(-6)}`

  await page.goto('/notes')
  await page.getByRole('button', { name: 'New note' }).click()
  const editor = page.getByRole('form', { name: 'Note editor' })
  await editor.getByLabel('Title').fill(noteTitle)
  await editor
    .getByLabel('Markdown')
    .fill('A **searchable launch note**.\n\n<img src=x onerror="window.noteXss=true">')
  await editor.getByRole('button', { name: 'Save note' }).click()
  await expect(page.getByRole('heading', { name: noteTitle })).toBeVisible()
  await expect(page.getByText('searchable launch note', { exact: false })).toBeVisible()
  await expect(page.locator('[onerror]')).toHaveCount(0)

  if (!test.info().project.name.includes('mobile')) {
    await page.getByPlaceholder('Search workspace').fill('searchable launch')
    await expect(page.getByRole('button').filter({ hasText: noteTitle })).toBeVisible()
  }

  page.once('dialog', (dialog) => dialog.accept())
  await page.getByRole('button', { name: 'Delete note' }).click()
  await expect(page.getByText(noteTitle, { exact: true })).toHaveCount(0)
})

test('creates and edits a standalone task with an automatic deadline', async ({ page }) => {
  const taskName = `Standalone task ${test.info().project.name}-${Date.now().toString().slice(-6)}`
  const editedTaskName = `Edited ${taskName}`
  let cleanupName = taskName

  try {
    await page.goto('/')
    await expect(
      page.getByRole('heading', { name: 'Make today count.', exact: true }),
    ).toBeVisible()
    await page.keyboard.press('n')
    const dialog = page.getByRole('dialog', { name: 'New task' })
    await expect(dialog).toBeVisible()
    await expect(dialog.getByLabel('Task title')).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(dialog).toBeHidden()
    await page.keyboard.press('n')
    await expect(dialog).toBeVisible()
    await expect(dialog.getByLabel('Task title')).toBeFocused()
    await dialog.getByLabel('Project').selectOption('')
    await dialog.getByLabel('Task title').fill(`${taskName} tomorrow`)
    await expect(dialog.locator('p').filter({ hasText: 'Deadline detected:' })).toBeVisible()
    await expect(dialog.getByLabel('Deadline', { exact: true })).not.toHaveValue('')
    await page.keyboard.press('Control+Enter')
    await expect(dialog).toBeHidden()
    const createdTask = page.getByText(taskName, { exact: true }).locator('xpath=ancestor::article')
    await expect(createdTask).toBeVisible()
    await expect(createdTask.getByText('Tomorrow', { exact: true })).toBeVisible()
    await createdTask.getByRole('button', { name: `Focus ${taskName} today` }).click()
    await expect(
      createdTask.getByRole('button', { name: `Remove ${taskName} from today's focus` }),
    ).toBeVisible()

    await page.getByRole('button', { name: `Edit ${taskName}` }).click()
    const editor = page.getByRole('form', { name: `Edit ${taskName}` })
    await editor.getByLabel('Task title').fill(editedTaskName)
    await editor.getByLabel('Edit priority').selectOption('high')
    await editor.getByLabel('Edit status').selectOption('blocked')
    await editor.getByRole('button', { name: 'Save', exact: true }).click()
    cleanupName = editedTaskName

    const editedTask = page
      .getByText(editedTaskName, { exact: true })
      .locator('xpath=ancestor::article')
    await expect(editedTask).toBeVisible()
    await expect(
      editedTask.locator('span.font-medium').filter({ hasText: 'Blocked' }),
    ).toBeVisible()
  } finally {
    await removeTestTask(page, cleanupName)
  }
})

test('manages descriptions, labels, reminders, and subtasks', async ({ page }) => {
  const suffix = `${test.info().project.name}-${Date.now().toString().slice(-6)}`
  const taskName = `Detailed task ${suffix}`
  const subtaskName = `Detailed subtask ${suffix}`
  const due = new Date()
  due.setDate(due.getDate() + 1)
  const reminder = new Date(Date.now() - 60_000)

  try {
    await page.goto('/')
    await expect(
      page.getByRole('heading', { name: 'Make today count.', exact: true }),
    ).toBeVisible()
    await page.keyboard.press('n')
    const dialog = page.getByRole('dialog', { name: 'New task' })
    await dialog.getByLabel('Task title').fill(taskName)
    await dialog.getByLabel('Deadline', { exact: true }).fill(localDateKey(due))
    await dialog.getByRole('button', { name: 'Details' }).click()
    await dialog.getByLabel('Description').fill('Context that survives the quick capture flow')
    await dialog.getByLabel('Reminder').fill(localDateTimeKey(reminder))
    await dialog.getByLabel('Labels').fill('E2E, Review')
    await dialog.getByRole('button', { name: 'Create task', exact: true }).click()
    await expect(dialog).toBeHidden()

    const task = page.getByText(taskName, { exact: true }).locator('xpath=ancestor::article')
    await expect(task).toContainText('Context that survives the quick capture flow')
    await expect(task.getByText('e2e', { exact: true })).toBeVisible()
    await expect(task.getByText('review', { exact: true })).toBeVisible()
    await expect(page.getByRole('status').filter({ hasText: taskName })).toBeVisible()
    await page.getByRole('button', { name: `Dismiss reminder for ${taskName}` }).click()

    await task.getByRole('button', { name: `Add subtask to ${taskName}` }).click()
    await task.getByPlaceholder('Subtask title').fill(subtaskName)
    await task.getByRole('button', { name: 'Add', exact: true }).click()
    await expect(page.getByText(subtaskName, { exact: true })).toBeVisible()

    await page.goto('/projects')
    await page.getByPlaceholder('Search tasks').fill(taskName)
    await expect(page.getByText(taskName, { exact: true })).toBeVisible()
    await expect(page.getByText(subtaskName, { exact: true })).toHaveCount(0)
    await page.getByLabel('Filter by label').selectOption('e2e')
    await expect(page.getByText(taskName, { exact: true })).toBeVisible()
  } finally {
    await removeTestTasks(page, [taskName, subtaskName])
  }
})

test('creates the next recurring task when an occurrence is completed', async ({ page }) => {
  const taskName = `Recurring task ${test.info().project.name}-${Date.now().toString().slice(-6)}`
  const due = new Date()
  due.setDate(due.getDate() + 1)

  try {
    await page.goto('/')
    await expect(
      page.getByRole('heading', { name: 'Make today count.', exact: true }),
    ).toBeVisible()
    await page.keyboard.press('n')
    const dialog = page.getByRole('dialog', { name: 'New task' })
    await dialog.getByLabel('Task title').fill(taskName)
    await dialog.getByLabel('Deadline', { exact: true }).fill(localDateKey(due))
    await dialog.getByRole('button', { name: 'Details' }).click()
    await dialog.getByLabel('Repeat').selectOption('daily')
    await dialog.getByRole('button', { name: 'Create task', exact: true }).click()
    await expect(dialog).toBeHidden()

    const occurrence = page.getByRole('article').filter({ hasText: taskName }).first()
    await expect(occurrence.getByText('daily', { exact: true })).toBeVisible()
    await occurrence.getByRole('button', { name: `Complete ${taskName}` }).click()

    await page.goto('/projects')
    await page.getByPlaceholder('Search tasks').fill(taskName)
    await expect(page.getByRole('button', { name: `Complete ${taskName}` })).toBeVisible()
    await expect(page.getByText('daily', { exact: true })).toBeVisible()
    await page.getByLabel('Filter by status').selectOption('completed')
    await expect(page.getByRole('button', { name: `Mark ${taskName} incomplete` })).toBeVisible()
  } finally {
    await removeTestTasks(page, [taskName])
  }
})

test('creates a project and completes its first task', async ({ page }) => {
  const suffix = `${test.info().project.name}-${Date.now().toString().slice(-6)}`
  const projectName = `E2E journey ${suffix}`
  const taskName = `Complete vertical slice ${suffix}`
  const due = new Date()
  due.setDate(due.getDate() + 1)
  const dueDate = localDateKey(due)

  try {
    await page.goto('/')
    await expect(page.getByRole('heading', { name: 'Make today count.' })).toBeVisible()

    await page.getByRole('main').getByRole('button', { name: 'New project', exact: true }).click()
    await page.getByLabel('Project name').fill(projectName)
    await page.getByLabel('Desired outcome').fill('Verify the complete browser workflow')
    const projectCreated = page.waitForResponse(
      (response) =>
        response.url().endsWith('/api/v1/projects') &&
        response.request().method() === 'POST' &&
        response.status() === 201,
    )
    await page.getByRole('button', { name: 'Create project', exact: true }).click()
    await projectCreated

    await page.goto('/projects')
    await page
      .getByRole('main')
      .getByRole('button', { name: new RegExp(projectName) })
      .click()
    await expect(page.getByRole('heading', { name: projectName })).toBeVisible()

    await page.getByRole('button', { name: 'Edit project' }).click()
    const projectEditor = page.getByRole('form', { name: 'Edit project' })
    await projectEditor
      .getByLabel('Desired outcome')
      .fill('Verify editing, progress, archive, and restore workflows')
    await projectEditor.getByLabel('Status').selectOption('active')
    await projectEditor.getByRole('button', { name: 'Save project' }).click()
    await expect(
      page.getByText('Verify editing, progress, archive, and restore workflows'),
    ).toBeVisible()

    await page.getByLabel('Task title').fill(taskName)
    await page.getByLabel('Deadline', { exact: true }).fill(dueDate)
    await page.getByRole('button', { name: 'Create task', exact: true }).click()
    await expect(page.getByText(taskName, { exact: true })).toBeVisible()

    await page.goto(`/calendar?date=${dueDate}`)
    const agenda = page.locator('section').last()
    await expect(agenda.getByText(taskName, { exact: true })).toBeVisible()
    await agenda.getByRole('button', { name: `Complete ${taskName}` }).click()
    await expect(agenda.getByRole('button', { name: `Mark ${taskName} incomplete` })).toBeVisible()

    await page.goto('/projects')
    await page
      .getByRole('main')
      .getByRole('button', { name: new RegExp(projectName) })
      .click()
    await expect(page.getByText('1 of 1 tasks complete', { exact: true })).toBeVisible()
    await page.getByLabel('Filter by status').selectOption('completed')
    await expect(page.getByText(taskName, { exact: true })).toBeVisible()

    await page.getByRole('button', { name: 'Archive project' }).click()
    await expect(page.getByText('archived', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Projects', exact: true }).click()
    await expect(page.getByRole('main').getByText(projectName, { exact: true })).toHaveCount(0)
    await page.getByRole('button', { name: /Show archived/ }).click()
    await page
      .getByRole('main')
      .getByRole('button', { name: new RegExp(projectName) })
      .click()
    await page.getByRole('button', { name: 'Restore project' }).click()
    await expect(page.getByText('active', { exact: true })).toBeVisible()

    page.once('dialog', (dialog) => dialog.accept())
    await page.getByRole('button', { name: 'Delete project' }).click()
    await expect(page.getByRole('heading', { name: 'Projects', level: 1 })).toBeVisible()
    await expect(page.getByRole('main').getByText(projectName, { exact: true })).toHaveCount(0)
  } finally {
    await removeTestProject(page, projectName)
  }
})

function localDateKey(value: Date) {
  const year = value.getFullYear()
  const month = String(value.getMonth() + 1).padStart(2, '0')
  const day = String(value.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function localDateTimeKey(value: Date) {
  return `${localDateKey(value)}T${String(value.getHours()).padStart(2, '0')}:${String(
    value.getMinutes(),
  ).padStart(2, '0')}`
}

async function removeTestTask(page: Page, taskName: string) {
  await removeTestTasks(page, [taskName])
}

async function removeTestTasks(page: Page, taskNames: string[]) {
  const userId = await page
    .evaluate(() => localStorage.getItem('prosepect.development-user-id'))
    .catch(() => null)
  if (!userId) return

  const headers = { 'x-prosepect-user-id': userId }
  const tasksResponse = await page.request.get('/api/v1/tasks?limit=100', { headers })
  if (!tasksResponse.ok()) return
  const body = (await tasksResponse.json()) as {
    items: Array<{ id: string; parent_task_id: string | null; title: string; version: number }>
  }
  const tasks = body.items
    .filter((candidate) => taskNames.includes(candidate.title))
    .sort(
      (first, second) =>
        Number(Boolean(second.parent_task_id)) - Number(Boolean(first.parent_task_id)),
    )

  for (const task of tasks) {
    await page.request.delete(`/api/v1/tasks/${task.id}?expected_version=${task.version}`, {
      headers,
    })
  }
}

async function removeTestProject(page: Page, projectName: string) {
  const userId = await page
    .evaluate(() => localStorage.getItem('prosepect.development-user-id'))
    .catch(() => null)
  if (!userId) return

  const headers = { 'x-prosepect-user-id': userId }
  const projectsResponse = await page.request.get('/api/v1/projects?limit=100', { headers })
  if (!projectsResponse.ok()) return
  const projects = (await projectsResponse.json()) as {
    items: Array<{ id: string; name: string; version: number }>
  }
  const project = projects.items.find((candidate) => candidate.name === projectName)
  if (!project) return

  const tasksResponse = await page.request.get(`/api/v1/tasks?project_id=${project.id}&limit=100`, {
    headers,
  })
  if (tasksResponse.ok()) {
    const tasks = (await tasksResponse.json()) as {
      items: Array<{ id: string; version: number }>
    }
    for (const task of tasks.items) {
      await page.request.delete(`/api/v1/tasks/${task.id}?expected_version=${task.version}`, {
        headers,
      })
    }
  }

  await page.request.delete(`/api/v1/projects/${project.id}?expected_version=${project.version}`, {
    headers,
  })
}
