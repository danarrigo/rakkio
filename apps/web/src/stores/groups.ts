import { computed, onScopeDispose, ref, watch } from 'vue'
import { defineStore } from 'pinia'
import * as api from '../api/client'
import type { GroupSummary, GroupDetail, GroupInbox } from '../api/types'
import { useWorkspaceStore } from './workspace'

// This value belongs to the disclosure shipped in this UI, never a future server response.
export const GROUP_ROSTER_NOTICE_VERSION = '2026-11-04'

export const useGroupsStore = defineStore('groups', () => {
  const workspace = useWorkspaceStore()
  const enabled = ref(false)
  const known = ref(false)
  const items = ref<GroupSummary[]>([])
  const inbox = ref<GroupInbox>({ invitations: [], requests: [] })
  const selectedId = ref<string | null>(null)
  const detail = ref<GroupDetail | null>(null)
  const loading = ref(false)
  const busy = ref(false)
  const stale = ref(true)
  const error = ref('')
  const message = ref('')
  const pendingCredential = ref<string | null>(null)
  let invitationTimeout: ReturnType<typeof setTimeout> | undefined
  function forgetInvitation() {
    pendingCredential.value = null
    clearTimeout(invitationTimeout)
    try {
      sessionStorage.removeItem('rakkio.pending-group-invite')
    } catch {
      /* Storage may be disabled. */
    }
  }
  function rememberInvitation(credential: string, expires = Date.now() + 15 * 60 * 1000) {
    if (
      !/^[a-zA-Z0-9_-]{43}$/.test(credential) ||
      expires <= Date.now() ||
      expires > Date.now() + 15 * 60 * 1000
    )
      return false
    pendingCredential.value = credential
    try {
      sessionStorage.setItem('rakkio.pending-group-invite', JSON.stringify({ credential, expires }))
    } catch {
      /* Keep only in memory if storage is disabled. */
    }
    clearTimeout(invitationTimeout)
    invitationTimeout = setTimeout(forgetInvitation, expires - Date.now())
    return true
  }
  try {
    const saved = JSON.parse(sessionStorage.getItem('rakkio.pending-group-invite') || 'null')
    if (
      !saved ||
      typeof saved.credential !== 'string' ||
      typeof saved.expires !== 'number' ||
      !rememberInvitation(saved.credential, saved.expires)
    )
      forgetInvitation()
  } catch {
    forgetInvitation()
  }
  const canWrite = computed(
    () =>
      enabled.value &&
      known.value &&
      !stale.value &&
      !loading.value &&
      !busy.value &&
      !!workspace.user,
  )
  let generation = 0
  let alive = true
  let controller: AbortController | undefined
  let timeout: ReturnType<typeof setTimeout> | undefined
  function cancel() {
    generation++
    controller?.abort()
    clearTimeout(timeout)
  }
  function begin() {
    cancel()
    const version = generation
    const owner = workspace.user?.id
    const request = new AbortController()
    controller = request
    timeout = setTimeout(() => request.abort(), 20000)
    return {
      signal: request.signal,
      owner,
      current: () => alive && version === generation && owner === workspace.user?.id,
    }
  }
  async function read(request: ReturnType<typeof begin>) {
    const [groups, invitations] = await Promise.all([
      api.listGroups(request.signal),
      api.getGroupInbox(request.signal),
    ])
    if (!request.current()) return
    items.value = groups
    inbox.value = invitations
    const selected = groups.find((group) => group.id === selectedId.value && !group.deleted_at)
    if (!selected) {
      selectedId.value = null
      detail.value = null
      return
    }
    try {
      const value = await api.getGroup(selected.id, request.signal)
      if (request.current()) detail.value = value
    } catch (cause) {
      if (request.current() && cause instanceof api.ApiError && [403, 404].includes(cause.status)) {
        items.value = items.value.filter((group) => group.id !== selected.id)
        detail.value = null
        selectedId.value = null
      }
      throw cause
    }
  }
  async function refresh() {
    if (!workspace.user || busy.value) return
    const request = begin()
    loading.value = true
    error.value = ''
    try {
      const capability = await api.getGroupsCapability(request.signal)
      if (!request.current()) return
      known.value = true
      enabled.value = capability.enabled === true
      if (enabled.value) await read(request)
      else {
        items.value = []
        detail.value = null
        selectedId.value = null
        inbox.value = { invitations: [], requests: [] }
      }
      if (request.current()) stale.value = false
    } catch (cause) {
      if (request.current()) {
        stale.value = true
        error.value =
          cause instanceof Error ? cause.message : 'Groups could not be loaded. Refresh to retry.'
      }
    } finally {
      if (request.current()) {
        loading.value = false
        clearTimeout(timeout)
      }
    }
  }
  async function select(id: string) {
    if (busy.value || loading.value) return
    selectedId.value = id
    detail.value = null
    await refresh()
  }
  async function write<T>(
    operation: (owner: string, signal: AbortSignal) => Promise<T>,
  ): Promise<T | undefined> {
    if (!canWrite.value || !workspace.user) return undefined
    const request = begin()
    if (!request.owner) return undefined
    busy.value = true
    error.value = ''
    message.value = ''
    let committed = false
    let result: T | undefined
    try {
      result = await operation(request.owner, request.signal)
      if (!request.current()) return undefined
      committed = true
      await read(request)
      if (request.current()) message.value = 'Saved.'
    } catch (cause) {
      if (request.current()) {
        stale.value = true
        if (cause instanceof api.ApiError && [401, 403, 404].includes(cause.status)) {
          items.value = []
          inbox.value = { invitations: [], requests: [] }
          detail.value = null
          selectedId.value = null
        }
        error.value = `${committed ? 'Change saved, but refreshing failed.' : 'Request not confirmed.'} ${cause instanceof Error ? cause.message : ''} Refresh before making more changes.`
      }
    } finally {
      if (request.current()) {
        busy.value = false
        clearTimeout(timeout)
      }
    }
    return request.current() ? result : undefined
  }
  watch(
    () => workspace.user?.id,
    (owner, previous) => {
      if (previous && owner !== previous) forgetInvitation()
      cancel()
      enabled.value = false
      known.value = false
      items.value = []
      inbox.value = { invitations: [], requests: [] }
      selectedId.value = null
      detail.value = null
      loading.value = false
      busy.value = false
      stale.value = true
      error.value = ''
      message.value = ''
      void refresh()
    },
    { immediate: true, flush: 'sync' },
  )
  onScopeDispose(() => {
    alive = false
    cancel()
    clearTimeout(invitationTimeout)
  })
  return {
    enabled,
    known,
    items,
    inbox,
    selectedId,
    detail,
    loading,
    busy,
    stale,
    error,
    message,
    canWrite,
    pendingCredential,
    rememberInvitation,
    forgetInvitation,
    refresh,
    select,
    write,
  }
})
