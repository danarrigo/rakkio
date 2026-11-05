import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { effectScope, ref, type EffectScope } from 'vue'
import { flushPromises } from '@vue/test-utils'
import * as api from '../api/client'
import type { GroupAssignment, GroupDetail, GroupProgressPreferences } from '../api/types'
import { useGroupAssignments } from './group-assignments'
import { useWorkspaceStore } from './workspace'

vi.mock('../api/client', async (original) => ({
  ...(await original<typeof import('../api/client')>()),
  getGroupsCapability: vi.fn(),
  getGroup: vi.fn(),
  listGroupAssignments: vi.fn(),
  getGroupAssignment: vi.fn(),
  getGroupProgressPreferences: vi.fn(),
  getGroupContentUsage: vi.fn(),
  listGroupAssignmentUndos: vi.fn(),
  getGroupAssignmentReport: vi.fn(),
}))
const assignment: GroupAssignment = {
  id: 'work',
  group_id: 'class',
  title: 'Homework',
  description: '',
  due_on: null,
  due_time: null,
  due_at: null,
  timezone: 'UTC',
  state: 'open',
  version: 1,
  my_progress: { version: 0, completed_at: null, seen_assignment_version: null },
  changed_since_completion: false,
}
const detail = (member = false): GroupDetail => ({
  group: {
    id: 'class',
    owner_id: 'owner',
    name: 'Class',
    timezone: 'UTC',
    approval_required: true,
    version: 1,
    deleted_at: null,
    undo_until: null,
    my_role: member ? 'member' : 'owner',
    my_membership_id: 'membership',
    my_membership_version: 1,
    member_count: 2,
  },
  members: [],
  removed_members: [],
  invitations: [],
  join_requests: [],
  ownership_offer: null,
})
const preferences: GroupProgressPreferences = {
  membership_id: 'membership',
  share_progress: false,
  version: 1,
  notice_version: null,
  shared_at: null,
}
let scope: EffectScope
let store: ReturnType<typeof useGroupAssignments>
const id = ref('class')
function start() {
  store = scope.run(() => useGroupAssignments(id))!
  return flushPromises()
}
beforeEach(() => {
  vi.resetAllMocks()
  setActivePinia(createPinia())
  scope = effectScope()
  id.value = 'class'
  useWorkspaceStore().user = {
    id: 'owner',
    display_name: 'Owner',
    email: 'private@example.test',
    timezone: 'UTC',
    avatar_url: null,
  }
  vi.mocked(api.getGroupsCapability).mockResolvedValue({ enabled: true })
  vi.mocked(api.getGroup).mockResolvedValue(detail())
  vi.mocked(api.listGroupAssignments).mockResolvedValue({ items: [assignment], next_cursor: null })
  vi.mocked(api.getGroupAssignment).mockResolvedValue(assignment)
  vi.mocked(api.getGroupProgressPreferences).mockResolvedValue(preferences)
  vi.mocked(api.getGroupContentUsage).mockResolvedValue({
    used_bytes: 1024,
    used_records: 3,
    max_bytes: 16777216,
    max_records: 20000,
  })
  vi.mocked(api.listGroupAssignmentUndos).mockResolvedValue({ items: [], next_cursor: null })
  vi.mocked(api.getGroupAssignmentReport).mockResolvedValue({ items: [] })
})
afterEach(() => {
  scope.stop()
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('scoped group assignments', () => {
  it('does not load group data behind a disabled or unknown gate', async () => {
    vi.mocked(api.getGroupsCapability).mockResolvedValueOnce({ enabled: false })
    await start()
    expect(store.enabled.value).toBe(false)
    expect(api.getGroup).not.toHaveBeenCalled()
    vi.mocked(api.getGroupsCapability).mockRejectedValueOnce(new Error('Unavailable'))
    await store.refresh()
    expect(store.canWrite.value).toBe(false)
    expect(api.listGroupAssignments).not.toHaveBeenCalled()
  })
  it('starts with open work and queries closed history independently', async () => {
    await start()
    expect(api.listGroupAssignments).toHaveBeenCalledWith(
      'class',
      'open',
      undefined,
      expect.any(AbortSignal),
    )
    await store.changeState('closed')
    expect(api.listGroupAssignments).toHaveBeenLastCalledWith(
      'class',
      'closed',
      undefined,
      expect.any(AbortSignal),
    )
    expect(store.preferences.value?.share_progress).toBe(false)
  })
  it('blocks uncertain writes until explicit refresh without retrying them', async () => {
    await start()
    const operation = vi.fn().mockRejectedValue(new Error('Connection lost'))
    await store.write(operation)
    expect(store.stale.value).toBe(true)
    await store.write(operation)
    expect(operation).toHaveBeenCalledOnce()
    await store.refresh()
    expect(store.canWrite.value).toBe(true)
  })
  it('distinguishes a confirmed commit followed by refresh failure from an unconfirmed mutation', async () => {
    await start()
    vi.mocked(api.getGroup).mockRejectedValueOnce(new Error('Refresh unavailable'))
    const operation = vi.fn().mockResolvedValue(assignment)
    expect(await store.write(operation)).toEqual(assignment)
    expect(store.error.value).toContain('Change saved, but refreshing failed.')
    expect(store.canWrite.value).toBe(false)
    expect(operation).toHaveBeenCalledOnce()
  })
  it('clears cached shared data after lost permission but keeps the editing identity for a copyable draft', async () => {
    await start()
    store.edit(assignment)
    await store.showReport(assignment)
    expect(store.report.value).not.toBeNull()
    await store.write(() =>
      Promise.reject(new api.ApiError('Membership removed.', 403, 'forbidden')),
    )
    expect(store.group.value).toBeNull()
    expect(store.items.value).toEqual([])
    expect(store.preferences.value).toBeNull()
    expect(store.report.value).toBeNull()
    expect(store.latest.value).toBeNull()
    expect(store.editingId.value).toBe('work')
  })
  it('keeps a quota-rejected draft context without treating capacity as revoked membership', async () => {
    await start()
    store.edit(assignment)
    await store.write(() =>
      Promise.reject(new api.ApiError('Content limit reached.', 403, 'content_quota_exceeded')),
    )
    expect(store.group.value?.id).toBe('class')
    expect(store.latest.value?.id).toBe('work')
    expect(store.canWrite.value).toBe(false)
    await store.refresh()
    expect(store.canWrite.value).toBe(true)
  })
  it('aborts and discards late reads after an account change', async () => {
    let finish: ((value: GroupDetail) => void) | undefined
    vi.mocked(api.getGroup).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await start()
    const signal = vi.mocked(api.getGroup).mock.calls[0]?.[1]
    vi.mocked(api.getGroupsCapability).mockResolvedValue({ enabled: false })
    useWorkspaceStore().user = { ...useWorkspaceStore().user!, id: 'other' }
    await flushPromises()
    finish?.(detail())
    await flushPromises()
    expect(signal?.aborted).toBe(true)
    expect(store.group.value).toBeNull()
    expect(store.items.value).toEqual([])
  })
  it('discards late writes and private state after switching group routes', async () => {
    await start()
    store.edit(assignment)
    let finish: ((value: GroupAssignment) => void) | undefined, signal: AbortSignal | undefined
    const pending = store.write((_actor, _snapshot, request) => {
      signal = request
      return new Promise<GroupAssignment>((resolve) => {
        finish = resolve
      })
    })
    vi.mocked(api.getGroupsCapability).mockResolvedValue({ enabled: false })
    id.value = 'another-class'
    await flushPromises()
    finish?.(assignment)
    expect(await pending).toBeUndefined()
    expect(signal?.aborted).toBe(true)
    expect(store.editingId.value).toBeNull()
    expect(store.items.value).toEqual([])
    expect(store.message.value).toBe('')
  })
  it('never applies a late successful mutation to another signed-in account', async () => {
    await start()
    let finish: ((value: GroupAssignment) => void) | undefined
    const pending = store.write(
      () =>
        new Promise<GroupAssignment>((resolve) => {
          finish = resolve
        }),
    )
    vi.mocked(api.getGroupsCapability).mockResolvedValue({ enabled: false })
    useWorkspaceStore().user = { ...useWorkspaceStore().user!, id: 'different-account' }
    await flushPromises()
    finish?.(assignment)
    expect(await pending).toBeUndefined()
    expect(store.message.value).toBe('')
    expect(store.group.value).toBeNull()
    expect(store.items.value).toEqual([])
  })
  it('times out a write without retrying or leaving it enabled', async () => {
    await start()
    vi.useFakeTimers()
    const operation = vi.fn(
      (_actor: string, _snapshot: unknown, signal: AbortSignal) =>
        new Promise<GroupAssignment>((_resolve, reject) =>
          signal.addEventListener(
            'abort',
            () => reject(new DOMException('Timed out', 'AbortError')),
            { once: true },
          ),
        ),
    )
    const pending = store.write(operation)
    await vi.advanceTimersByTimeAsync(20001)
    await pending
    expect(store.busy.value).toBe(false)
    expect(store.stale.value).toBe(true)
    expect(store.canWrite.value).toBe(false)
    expect(operation).toHaveBeenCalledOnce()
  })
  it('does not request leader reports or receipts for ordinary members', async () => {
    vi.mocked(api.getGroup).mockResolvedValue(detail(true))
    await start()
    await store.showReport(assignment)
    expect(api.listGroupAssignmentUndos).not.toHaveBeenCalled()
    expect(api.getGroupAssignmentReport).not.toHaveBeenCalled()
  })
  it('drops reports immediately when refreshing a demoted role', async () => {
    await start()
    await store.showReport(assignment)
    expect(store.report.value).not.toBeNull()
    vi.mocked(api.getGroup).mockResolvedValue(detail(true))
    const pending = store.refresh()
    expect(store.report.value).toBeNull()
    await pending
    expect(store.leader.value).toBe(false)
  })
  it('refreshes a conflicted draft by exact identity rather than its current list page', async () => {
    await start()
    store.edit(assignment)
    vi.mocked(api.listGroupAssignments).mockResolvedValue({ items: [], next_cursor: 'older' })
    vi.mocked(api.getGroupAssignment).mockResolvedValue({
      ...assignment,
      title: 'Current definition',
      version: 2,
    })
    await store.refresh()
    expect(store.items.value).toEqual([])
    expect(store.latest.value?.version).toBe(2)
    vi.mocked(api.getGroupAssignment).mockRejectedValueOnce(
      new api.ApiError('Deleted.', 404, 'not_found'),
    )
    await store.refresh()
    expect(store.latest.value).toBeNull()
    expect(store.editingId.value).toBe('work')
  })
  it('rejects mixed membership snapshots and stale repeated pagination cursors', async () => {
    vi.mocked(api.getGroupProgressPreferences).mockResolvedValueOnce({
      ...preferences,
      membership_id: 'fresh-membership',
    })
    await start()
    expect(store.group.value).toBeNull()
    expect(store.canWrite.value).toBe(false)
    vi.mocked(api.listGroupAssignments).mockResolvedValue({
      items: [assignment],
      next_cursor: 'same',
    })
    await store.refresh()
    await store.more()
    expect(store.stale.value).toBe(true)
    expect(store.error.value).toContain('Pagination did not advance')
  })
})
