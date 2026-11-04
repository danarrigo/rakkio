import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { flushPromises } from '@vue/test-utils'
import * as api from '../api/client'
import type { GroupDetail, GroupSummary } from '../api/types'
import { useGroupsStore } from './groups'
import { useWorkspaceStore } from './workspace'

vi.mock('../api/client', async (original) => ({
  ...(await original<typeof import('../api/client')>()),
  getGroupsCapability: vi.fn(),
  listGroups: vi.fn(),
  getGroupInbox: vi.fn(),
  getGroup: vi.fn(),
  mutateGroup: vi.fn(),
}))
const summary = (id = 'class'): GroupSummary => ({
  id,
  owner_id: 'owner',
  name: id,
  timezone: 'UTC',
  approval_required: true,
  version: 1,
  my_role: 'owner',
  my_membership_id: 'membership',
  my_membership_version: 1,
  member_count: 1,
  deleted_at: null,
  undo_until: null,
})
const detail = (id = 'class'): GroupDetail => ({
  group: summary(id),
  members: [],
  removed_members: [],
  invitations: [],
  join_requests: [],
  ownership_offer: null,
})
let store: ReturnType<typeof useGroupsStore> | undefined
beforeEach(() => {
  vi.resetAllMocks()
  sessionStorage.clear()
  setActivePinia(createPinia())
  useWorkspaceStore().user = {
    id: 'owner',
    display_name: 'Owner',
    email: 'owner@example.test',
    avatar_url: null,
    timezone: 'UTC',
  }
  vi.mocked(api.getGroupsCapability).mockResolvedValue({ enabled: true })
  vi.mocked(api.listGroups).mockResolvedValue([summary()])
  vi.mocked(api.getGroupInbox).mockResolvedValue({ invitations: [], requests: [] })
  vi.mocked(api.getGroup).mockResolvedValue(detail())
})
afterEach(() => {
  store?.$dispose()
  store = undefined
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('groups store', () => {
  it('does not fetch or mutate group data when the server gate is closed', async () => {
    vi.mocked(api.getGroupsCapability).mockResolvedValue({ enabled: false })
    store = useGroupsStore()
    await flushPromises()
    expect(store.known).toBe(true)
    expect(store.enabled).toBe(false)
    expect(api.listGroups).not.toHaveBeenCalled()
    const operation = vi.fn()
    await store.write(operation)
    expect(operation).not.toHaveBeenCalled()
  })
  it('does not treat unknown capability as permission to fetch groups', async () => {
    vi.mocked(api.getGroupsCapability).mockRejectedValueOnce(new Error('Unavailable.'))
    store = useGroupsStore()
    await flushPromises()
    expect(store.canWrite).toBe(false)
    expect(store.known).toBe(false)
    expect(api.listGroups).not.toHaveBeenCalled()
    await store.refresh()
    expect(store.enabled).toBe(true)
  })
  it('refreshes committed changes without repeating the mutation', async () => {
    store = useGroupsStore()
    await flushPromises()
    await store.select('class')
    vi.mocked(api.mutateGroup).mockResolvedValue({ group: summary() })
    const result = await store.write((owner, signal) =>
      api.mutateGroup(
        'class',
        { expected_user_id: owner, expected_version: 1, action: { kind: 'cancel_ownership' } },
        signal,
      ),
    )
    expect(result?.group?.id).toBe('class')
    expect(api.mutateGroup).toHaveBeenCalledOnce()
    expect(vi.mocked(api.mutateGroup).mock.calls[0]?.[1].expected_user_id).toBe('owner')
    expect(store.detail?.group.id).toBe('class')
  })
  it('blocks uncertain mutations until an explicit refresh', async () => {
    store = useGroupsStore()
    await flushPromises()
    const operation = vi.fn().mockRejectedValue(new Error('Connection interrupted.'))
    await store.write(operation)
    expect(store.stale).toBe(true)
    expect(store.canWrite).toBe(false)
    await store.write(operation)
    expect(operation).toHaveBeenCalledOnce()
    await store.refresh()
    expect(store.canWrite).toBe(true)
  })
  it('purges cached group data when permissions are rejected', async () => {
    store = useGroupsStore()
    await flushPromises()
    await store.select('class')
    await store.write(() =>
      Promise.reject(new api.ApiError('Membership removed.', 403, 'forbidden')),
    )
    expect(store.items).toEqual([])
    expect(store.detail).toBeNull()
    expect(store.selectedId).toBeNull()
    expect(store.canWrite).toBe(false)
  })
  it('discards late reads and writes after switching accounts', async () => {
    store = useGroupsStore()
    await flushPromises()
    let finish: ((value: { group: GroupSummary }) => void) | undefined
    let signal: AbortSignal | undefined
    const pending = store.write((_owner, request) => {
      signal = request
      return new Promise<{ group: GroupSummary }>((resolve) => {
        finish = resolve
      })
    })
    vi.mocked(api.listGroups).mockResolvedValue([summary('new-owner')])
    useWorkspaceStore().user = { ...useWorkspaceStore().user!, id: 'another' }
    await flushPromises()
    finish?.({ group: summary('old-owner') })
    await flushPromises()
    expect(await pending).toBeUndefined()
    expect(signal?.aborted).toBe(true)
    expect(store.items.map((item) => item.id)).toEqual(['new-owner'])
    expect(store.message).toBe('')
  })
  it('expires temporary invite credentials and clears them on logout', async () => {
    store = useGroupsStore()
    await flushPromises()
    vi.useFakeTimers()
    const credential = 'a'.repeat(43)
    expect(store.rememberInvitation(credential)).toBe(true)
    expect(store.pendingCredential).toBe(credential)
    await vi.advanceTimersByTimeAsync(15 * 60 * 1000)
    expect(store.pendingCredential).toBeNull()
    expect(sessionStorage.getItem('rakkio.pending-group-invite')).toBeNull()
    store.rememberInvitation(credential)
    useWorkspaceStore().user = null
    expect(store.pendingCredential).toBeNull()
    expect(store.rememberInvitation('invalid')).toBe(false)
  })
  it('keeps invitation handling functional when browser storage is disabled', async () => {
    store = useGroupsStore()
    await flushPromises()
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('Disabled')
    })
    vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
      throw new Error('Disabled')
    })
    expect(store.rememberInvitation('a'.repeat(43))).toBe(true)
    expect(() => store!.forgetInvitation()).not.toThrow()
  })
})
