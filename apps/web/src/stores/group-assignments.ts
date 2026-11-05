import { computed, onScopeDispose, ref, toValue, watch, type MaybeRefOrGetter } from 'vue'
import * as api from '../api/client'
import type {
  AssignmentProgressReport,
  GroupAssignment,
  GroupAssignmentUndo,
  GroupContentUsage,
  GroupProgressPreferences,
  GroupSummary,
} from '../api/types'
import { useWorkspaceStore } from './workspace'

// Pin this to the disclosure shipped in the view, never a server-supplied version.
export const GROUP_PROGRESS_NOTICE_VERSION = '2026-11-04-progress'
export function useGroupAssignments(groupId: MaybeRefOrGetter<string>) {
  const workspace = useWorkspaceStore()
  const group = ref<GroupSummary | null>(null)
  const items = ref<GroupAssignment[]>([])
  const preferences = ref<GroupProgressPreferences | null>(null)
  const usage = ref<GroupContentUsage | null>(null)
  const undos = ref<GroupAssignmentUndo[]>([])
  const next = ref<string | null>(null),
    nextUndo = ref<string | null>(null)
  const state = ref<'open' | 'closed'>('open')
  const editingId = ref<string | null>(null),
    latest = ref<GroupAssignment | null>(null)
  const report = ref<{ assignment: GroupAssignment; data: AssignmentProgressReport } | null>(null)
  const loading = ref(false),
    busy = ref(false),
    stale = ref(true),
    enabled = ref(false)
  const error = ref(''),
    message = ref('')
  const leader = computed(() => ['owner', 'leader'].includes(group.value?.my_role || ''))
  const canWrite = computed(
    () =>
      !!workspace.user &&
      enabled.value &&
      !!group.value &&
      !!preferences.value &&
      !stale.value &&
      !busy.value &&
      !loading.value,
  )
  let generation = 0,
    alive = true
  let controller: AbortController | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  function cancel() {
    generation++
    controller?.abort()
    clearTimeout(timer)
  }
  function begin() {
    cancel()
    const version = generation,
      owner = workspace.user?.id,
      id = toValue(groupId)
    const request = new AbortController()
    controller = request
    timer = setTimeout(() => request.abort(), 20000)
    report.value = null
    return {
      owner,
      id,
      signal: request.signal,
      current: () =>
        alive && version === generation && owner === workspace.user?.id && id === toValue(groupId),
    }
  }
  function clearData() {
    group.value = null
    items.value = []
    preferences.value = null
    usage.value = null
    undos.value = []
    next.value = null
    nextUndo.value = null
    report.value = null
    latest.value = null
  }
  function failure(cause: unknown, committed = false) {
    controller?.abort()
    stale.value = true
    if (
      cause instanceof api.ApiError &&
      [401, 403, 404, 503].includes(cause.status) &&
      cause.code !== 'content_quota_exceeded'
    )
      clearData()
    error.value = `${committed ? 'Change saved, but refreshing failed.' : 'Request not confirmed.'} ${cause instanceof Error ? cause.message : 'The server could not be reached.'} Refresh before making more changes.`
  }
  async function read(request: ReturnType<typeof begin>) {
    const capability = await api.getGroupsCapability(request.signal)
    if (!request.current()) return
    request.signal.throwIfAborted()
    enabled.value = capability.enabled === true
    if (!enabled.value) {
      clearData()
      return
    }
    const value = await api.getGroup(request.id, request.signal)
    if (!request.current()) return
    request.signal.throwIfAborted()
    if (value.group.deleted_at) {
      clearData()
      throw new Error('This group is deleted. Manage group recovery from Groups.')
    }
    const isLeader = ['owner', 'leader'].includes(value.group.my_role)
    const edit = editingId.value
    const [page, prefs, budget, deleted, current] = await Promise.all([
      api.listGroupAssignments(request.id, state.value, undefined, request.signal),
      api.getGroupProgressPreferences(request.id, request.signal),
      api.getGroupContentUsage(request.id, request.signal),
      isLeader
        ? api.listGroupAssignmentUndos(request.id, undefined, request.signal)
        : Promise.resolve({ items: [], next_cursor: null }),
      edit
        ? api.getGroupAssignment(request.id, edit, request.signal).catch((cause) => {
            if (cause instanceof api.ApiError && cause.status === 404) return null
            throw cause
          })
        : Promise.resolve(null),
    ])
    if (!request.current()) return
    request.signal.throwIfAborted()
    if (prefs.membership_id !== value.group.my_membership_id) {
      clearData()
      throw new Error('Your membership changed during refresh.')
    }
    group.value = value.group
    items.value = page.items
    next.value = page.next_cursor ?? null
    preferences.value = prefs
    usage.value = budget
    undos.value = deleted.items
    nextUndo.value = deleted.next_cursor ?? null
    if (edit === editingId.value) latest.value = current
  }
  async function refresh() {
    if (!workspace.user || busy.value || !toValue(groupId)) return
    const request = begin()
    loading.value = true
    stale.value = true
    error.value = ''
    message.value = ''
    try {
      await read(request)
      if (request.current()) stale.value = false
    } catch (cause) {
      if (request.current()) failure(cause)
    } finally {
      if (request.current()) {
        loading.value = false
        clearTimeout(timer)
      }
    }
  }
  async function write<T>(
    operation: (actor: string, snapshot: GroupSummary, signal: AbortSignal) => Promise<T>,
    success = 'Saved.',
  ): Promise<T | undefined> {
    if (!canWrite.value || !group.value || !workspace.user) return undefined
    const snapshot = group.value,
      request = begin()
    if (!request.owner) return undefined
    busy.value = true
    error.value = ''
    message.value = ''
    let result: T | undefined,
      committed = false
    try {
      const value = await operation(request.owner, snapshot, request.signal)
      if (!request.current()) return undefined
      request.signal.throwIfAborted()
      result = value
      committed = true
      await read(request)
      if (request.current()) {
        stale.value = false
        message.value = success
      }
    } catch (cause) {
      if (request.current()) failure(cause, committed)
    } finally {
      if (request.current()) {
        busy.value = false
        clearTimeout(timer)
      }
    }
    return request.current() ? result : undefined
  }
  async function more(deleted = false) {
    const cursor = deleted ? nextUndo.value : next.value
    if (!canWrite.value || !cursor || (deleted && !leader.value)) return
    const request = begin()
    loading.value = true
    error.value = ''
    try {
      if (deleted) {
        const page = await api.listGroupAssignmentUndos(request.id, cursor, request.signal)
        if (!request.current()) return
        request.signal.throwIfAborted()
        if (page.next_cursor === cursor) throw new Error('Pagination did not advance.')
        undos.value = [
          ...new Map([...undos.value, ...page.items].map((row) => [row.id, row])).values(),
        ]
        nextUndo.value = page.next_cursor ?? null
      } else {
        const page = await api.listGroupAssignments(request.id, state.value, cursor, request.signal)
        if (!request.current()) return
        request.signal.throwIfAborted()
        if (page.next_cursor === cursor) throw new Error('Pagination did not advance.')
        items.value = [
          ...new Map([...items.value, ...page.items].map((row) => [row.id, row])).values(),
        ]
        next.value = page.next_cursor ?? null
      }
    } catch (cause) {
      if (request.current()) failure(cause)
    } finally {
      if (request.current()) {
        loading.value = false
        clearTimeout(timer)
      }
    }
  }
  async function changeState(value: 'open' | 'closed') {
    if (busy.value || loading.value || state.value === value) return
    state.value = value
    items.value = []
    next.value = null
    await refresh()
  }
  async function showReport(assignment: GroupAssignment) {
    if (!canWrite.value || !leader.value) return
    const request = begin()
    loading.value = true
    error.value = ''
    try {
      const data = await api.getGroupAssignmentReport(request.id, assignment.id, request.signal)
      if (!request.current()) return
      request.signal.throwIfAborted()
      report.value = { assignment, data }
    } catch (cause) {
      if (request.current()) failure(cause)
    } finally {
      if (request.current()) {
        loading.value = false
        clearTimeout(timer)
      }
    }
  }
  function edit(assignment: GroupAssignment | null) {
    editingId.value = assignment?.id ?? null
    latest.value = assignment
  }
  watch(
    [() => workspace.user?.id, () => toValue(groupId)],
    () => {
      cancel()
      clearData()
      editingId.value = null
      enabled.value = false
      busy.value = false
      loading.value = false
      stale.value = true
      state.value = 'open'
      error.value = ''
      message.value = ''
      void refresh()
    },
    { immediate: true, flush: 'sync' },
  )
  onScopeDispose(() => {
    alive = false
    cancel()
    clearData()
  })
  return {
    group,
    items,
    preferences,
    usage,
    undos,
    next,
    nextUndo,
    state,
    editingId,
    latest,
    report,
    loading,
    busy,
    stale,
    enabled,
    error,
    message,
    leader,
    canWrite,
    refresh,
    write,
    more,
    changeState,
    showReport,
    edit,
  }
}
