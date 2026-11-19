import { expect, test } from '@playwright/test'
import type { Task } from '../src/api/types'

// Date-only deadlines resolve to local end of day; the expected instants assume UTC.
test.use({ timezoneId: 'UTC' })

// Real isolated CI API, normal session cookies/CSRF, no intercepted responses.
test('real backend: one-off edits preserve cadence and forward edits preserve completed history', async ({
  page,
}) => {
  await page.goto('/inbox')
  await expect(page.getByRole('heading', { name: 'Inbox', exact: true })).toBeVisible()
  const session = await (await page.request.get('/api/v1/session')).json()
  const headers = { 'x-csrf-token': session.csrf_token as string }
  const suffix = `${test.info().project.name}-${Date.now()}`
  const originalTitle = `Routine ${suffix}`
  const onceTitle = `Exception ${suffix}`
  const futureTitle = `Future ${suffix}`
  const created = await page.request.post('/api/v1/tasks', {
    headers,
    data: { title: originalTitle, due_at: '2030-01-01T12:00:00Z', recurrence: 'daily' },
  })
  expect(created.status()).toBe(201)
  const original: Task = await created.json()
  try {
    await page.reload()
    await page.getByRole('button', { name: `Edit ${originalTitle}`, exact: true }).click()
    const one = page.getByRole('form', { name: `Edit ${originalTitle}`, exact: true })
    await expect(one.getByRole('combobox', { name: 'Apply changes to', exact: true })).toHaveValue(
      'this_occurrence',
    )
    await one.getByRole('textbox', { name: 'Task title', exact: true }).fill(onceTitle)
    await one.getByLabel('Deadline', { exact: true }).fill('2030-01-04')
    await one.getByRole('button', { name: 'Save', exact: true }).click()
    await page.getByRole('button', { name: `Complete ${onceTitle}`, exact: true }).click()
    await expect(
      page.getByRole('button', { name: `Edit ${originalTitle}`, exact: true }),
    ).toBeVisible()
    const firstList: { items: Task[] } = await (
      await page.request.get('/api/v1/tasks?limit=100')
    ).json()
    const completed = firstList.items.find((task) => task.id === original.id)!
    expect(completed).toMatchObject({
      title: onceTitle,
      status: 'completed',
      due_at: '2030-01-04T23:59:00Z',
    })
    const second = firstList.items.find((task) => task.title === originalTitle)!
    expect(second.due_at).toBe('2030-01-02T12:00:00Z')
    await page.getByRole('button', { name: `Edit ${originalTitle}`, exact: true }).click()
    const future = page.getByRole('form', { name: `Edit ${originalTitle}`, exact: true })
    await future
      .getByRole('combobox', { name: 'Apply changes to', exact: true })
      .selectOption('this_and_future')
    await future.getByRole('textbox', { name: 'Task title', exact: true }).fill(futureTitle)
    await future.getByRole('combobox', { name: 'Repeat', exact: true }).selectOption('weekly')
    await future.getByRole('button', { name: 'Save', exact: true }).click()
    await page.getByRole('button', { name: `Complete ${futureTitle}`, exact: true }).click()
    // A recurring completion removes its row and inserts the next one. Wait for
    // that canonical successor, not a button whose title both occurrences share.
    await expect
      .poll(async () => {
        const list: { items: Task[] } = await (
          await page.request.get('/api/v1/tasks?limit=100')
        ).json()
        return list.items.filter((task) => task.title === futureTitle).length
      })
      .toBe(2)
    await page.reload()
    const final: { items: Task[] } = await (
      await page.request.get('/api/v1/tasks?limit=100')
    ).json()
    expect(final.items.find((task) => task.id === original.id)).toEqual(completed)
    expect(
      final.items.find((task) => task.title === futureTitle && task.status === 'todo'),
    ).toMatchObject({ recurrence: 'weekly', due_at: '2030-01-09T12:00:00Z' })
    await expect(
      page.getByRole('button', { name: `Edit ${futureTitle}`, exact: true }),
    ).toBeVisible()
  } finally {
    const remaining: { items: Task[] } = await (
      await page.request.get('/api/v1/tasks?limit=100')
    ).json()
    for (const task of remaining.items.filter(
      (task) =>
        task.id === original.id || [originalTitle, onceTitle, futureTitle].includes(task.title),
    )) {
      const removed = await page.request.delete(`/api/v1/tasks/${task.id}`, {
        headers,
        params: { expected_version: task.version },
      })
      expect(removed.status(), await removed.text()).toBe(204)
    }
  }
})
