<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { RefreshCw } from '@lucide/vue'
import * as api from '../api/client'
import type {
  ActivityEntry,
  GoogleIntegrationStatus,
  SyncConflict,
  Synchronization,
} from '../api/types'
import { synchronizationActive, synchronizationMessage } from '../sync-status'

const integration = ref<GoogleIntegrationStatus | null>(null)
const conflicts = ref<SyncConflict[]>([])
const activity = ref<ActivityEntry[]>([])
const trackedJob = ref<Synchronization | null>(null)
const loading = ref(true)
const busy = ref(false)
const error = ref('')
const message = ref('')
let mounted = false
let timer: ReturnType<typeof setTimeout> | undefined
let request: AbortController | null = null
const job = computed(() => trackedJob.value ?? integration.value?.latest_synchronization ?? null)
const disconnecting = computed(
  () => job.value?.kind === 'credential_revoke' && synchronizationActive(job.value),
)
const disabled = computed(() => loading.value || busy.value || disconnecting.value)

function disconnectGoogle() {
  if (
    !window.confirm(
      'Disconnect Google? This revokes both Calendar and Tasks access. Existing Google Tasks copies and rakkio tasks are kept.',
    )
  )
    return
  void queue(api.revokeGoogleIntegration)
}

function scheduleRefresh() {
  clearTimeout(timer)
  if (mounted && !document.hidden) timer = setTimeout(() => void refresh(), 3000)
}

async function refresh() {
  if (request || busy.value || !mounted || document.hidden) return
  clearTimeout(timer)
  const controller = new AbortController()
  request = controller
  loading.value = true
  try {
    const [status, decisions, recentActivity, tracked] = await Promise.all([
      api.getGoogleIntegration(controller.signal),
      api.listSyncConflicts(controller.signal),
      api.listActivity(controller.signal),
      trackedJob.value ? api.getSynchronization(trackedJob.value.id, controller.signal) : null,
    ])
    if (!mounted || controller.signal.aborted) return
    integration.value = status
    conflicts.value = decisions
    activity.value = recentActivity
    trackedJob.value = tracked
    error.value = ''
  } catch {
    if (mounted && !controller.signal.aborted)
      error.value =
        'Could not refresh Google status. Check your connection and refresh status; no completion is confirmed.'
  } finally {
    request = null
    if (mounted) {
      loading.value = false
      scheduleRefresh()
    }
  }
}

async function queue(action: () => Promise<Synchronization>) {
  if (disabled.value) return
  clearTimeout(timer)
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    const queued = await action()
    if (mounted) trackedJob.value = queued
  } catch {
    if (mounted)
      error.value =
        'Could not confirm this request. Refresh status before trying again. If failures continue, reconnect Google and check calendar permissions.'
  } finally {
    if (mounted) {
      busy.value = false
      scheduleRefresh()
    }
  }
}

function deletionConflict(conflict: SyncConflict) {
  return conflict.allowed_resolutions.length === 1 && conflict.allowed_resolutions[0] === 'google'
}

async function resolve(conflict: SyncConflict, resolution: 'google' | 'prosepect' | 'latest') {
  if (disabled.value || !conflict.allowed_resolutions.includes(resolution)) return
  clearTimeout(timer)
  busy.value = true
  error.value = ''
  try {
    await api.resolveSyncConflict(conflict.id, resolution)
    if (!mounted) return
    conflicts.value = conflicts.value.filter((candidate) => candidate.id !== conflict.id)
    message.value = deletionConflict(conflict)
      ? 'Keep Google saved. The task stays deleted; reconciliation of the standalone Google event is queued.'
      : 'Decision saved. Applying it is queued; synchronization has not completed yet.'
  } catch (cause) {
    if (mounted)
      error.value =
        cause instanceof api.ApiError
          ? `${cause.message} Refresh status and try again.`
          : 'Could not save the conflict decision. Refresh status and try again.'
  } finally {
    if (mounted) {
      busy.value = false
      scheduleRefresh()
    }
  }
}

function visibilityChanged() {
  if (document.hidden) {
    clearTimeout(timer)
    request?.abort()
  } else void refresh()
}

onMounted(() => {
  mounted = true
  document.addEventListener('visibilitychange', visibilityChanged)
  void refresh()
})
onBeforeUnmount(() => {
  mounted = false
  clearTimeout(timer)
  request?.abort()
  document.removeEventListener('visibilitychange', visibilityChanged)
})
</script>

<template>
  <section
    class="border-b border-slate-200 py-8 dark:border-slate-800"
    aria-label="Google Calendar synchronization"
  >
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div>
        <h2 class="text-sm font-semibold">Google Calendar</h2>
        <p class="mt-2 text-sm text-slate-500 dark:text-slate-400">
          {{
            !integration
              ? loading
                ? 'Loading Google connection…'
                : 'Google connection status unavailable.'
              : integration.connected
                ? 'Connected with encrypted credentials.'
                : 'Not connected. Connect Google to synchronize calendars and scheduled tasks.'
          }}
        </p>
      </div>
      <a
        v-if="integration && !integration.connected && !disconnecting"
        class="primary-button"
        :href="api.apiUrl('/api/v1/auth/google/calendar/start')"
        >Connect Google</a
      >
      <div v-if="integration?.connected" class="flex flex-wrap gap-2">
        <button
          class="secondary-button"
          type="button"
          :disabled="disabled"
          @click="queue(api.discoverGoogleCalendars)"
        >
          <RefreshCw :size="14" /> Discover calendars
        </button>
        <button
          class="primary-button"
          type="button"
          :disabled="disabled || synchronizationActive(job)"
          @click="queue(() => api.synchronize())"
        >
          Sync now
        </button>
      </div>
    </div>
    <p class="mt-4 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
      Connecting lets rakkio view your Google calendar list and access roles and read, create,
      update, or delete events on calendars allowed by your Google permissions. rakkio stores
      synchronized event fields and encrypted OAuth credentials only to provide calendar and
      scheduled-task synchronization. It does not use Google data for advertising, sale, or AI
      training. You can disconnect at any time. See the
      <RouterLink class="underline" to="/privacy">Privacy Policy</RouterLink>.
    </p>
    <p v-if="job" role="status" class="mt-4 text-xs leading-5 text-slate-500">
      {{ synchronizationMessage(job) }}
    </p>
    <p v-if="integration?.pending_synchronization_count" class="mt-2 text-xs text-slate-500">
      {{ integration.pending_synchronization_count }} job(s) waiting, running, or retrying. A
      completed request does not mean all changes are synchronized.
    </p>
    <p
      v-if="integration?.failed_synchronization_count"
      class="mt-2 text-xs text-amber-700 dark:text-amber-400"
    >
      {{ integration.failed_synchronization_count }} job(s) stopped after repeated failures. Try the
      affected action again, or reconnect Google and check calendar permissions.
    </p>
    <p v-if="message" role="status" class="mt-3 text-xs text-slate-500">{{ message }}</p>
    <p v-if="error" role="alert" class="mt-3 text-xs text-rose-600">{{ error }}</p>
    <button
      class="mt-4 text-sm font-medium underline underline-offset-4 disabled:opacity-50"
      type="button"
      :disabled="loading || busy"
      @click="refresh"
    >
      {{ loading ? 'Refreshing status…' : 'Refresh status' }}
    </button>
    <div v-if="conflicts.length" class="mt-6">
      <h3 class="text-xs font-semibold uppercase tracking-wide text-amber-600">Needs a decision</h3>
      <p
        v-if="conflicts.some((conflict) => !deletionConflict(conflict))"
        class="mt-2 text-xs leading-5 text-slate-500"
      >
        Both copies changed. Choose which version to keep; the other version's changes will be
        replaced. A Google deletion removes an event or unschedules a task, not the task itself.
      </p>
      <div
        v-for="conflict in conflicts"
        :key="conflict.id"
        class="mt-2 flex flex-wrap items-center gap-2 border-t border-slate-100 py-3 dark:border-slate-900"
      >
        <div class="min-w-0 flex-1 text-sm">
          <p class="break-words">{{ conflict.title }}</p>
          <p
            v-if="deletionConflict(conflict)"
            class="mt-1 text-xs text-slate-500 dark:text-slate-400"
          >
            The task stays deleted. Keep Google keeps the current Google event as a standalone
            event, without restoring the task.
          </p>
        </div>
        <button
          v-if="conflict.allowed_resolutions.includes('google')"
          class="secondary-button !h-8 !text-xs"
          type="button"
          :disabled="disabled"
          @click="resolve(conflict, 'google')"
        >
          {{ deletionConflict(conflict) ? 'Keep Google' : 'Use Google' }}
        </button>
        <button
          v-if="conflict.allowed_resolutions.includes('prosepect')"
          class="secondary-button !h-8 !text-xs"
          type="button"
          :disabled="disabled"
          @click="resolve(conflict, 'prosepect')"
        >
          Use rakkio
        </button>
        <button
          v-if="conflict.allowed_resolutions.includes('latest')"
          class="secondary-button !h-8 !text-xs"
          type="button"
          :disabled="disabled"
          @click="resolve(conflict, 'latest')"
        >
          Use latest
        </button>
      </div>
    </div>
    <div v-if="activity.length" class="mt-6">
      <h3 class="text-xs font-semibold uppercase tracking-wide text-slate-400">Recent activity</h3>
      <p
        v-for="entry in activity.slice(0, 5)"
        :key="`${entry.created_at}:${entry.kind}`"
        class="mt-2 text-xs text-slate-500"
      >
        {{ entry.message }} ·
        {{
          new Date(entry.created_at).toLocaleString(undefined, {
            dateStyle: 'medium',
            timeStyle: 'short',
          })
        }}
      </p>
    </div>
    <div v-if="integration?.connected" class="mt-6 flex flex-wrap items-center gap-4 text-xs">
      <a
        v-if="!disconnecting"
        class="underline"
        :href="api.apiUrl('/api/v1/auth/google/calendar/start')"
        >Reconnect Google</a
      >
      <button
        class="text-rose-600 hover:underline"
        type="button"
        :disabled="disabled"
        @click="disconnectGoogle"
      >
        Disconnect Google
      </button>
    </div>
  </section>
</template>
