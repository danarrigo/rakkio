<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import * as api from '../api/client'
import type {
  GoogleTaskConflict,
  GoogleTaskRecovery,
  GoogleTaskResolutionRequest,
  GoogleTaskList,
  GoogleTasksStatus,
} from '../api/types'

const connectUrl = api.apiUrl('/api/v1/auth/google/tasks/start')
const status = ref<GoogleTasksStatus | null>(null)
const lists = ref<GoogleTaskList[]>([])
const conflicts = ref<GoogleTaskConflict[]>([])
const recoveries = ref<GoogleTaskRecovery[]>([])
const refreshButton = ref<HTMLButtonElement | null>(null)
const queuedChoices = ref(new Set<string>())
const selectedList = ref('')
const timezone = ref(Intl.DateTimeFormat().resolvedOptions().timeZone)
const loading = ref(true)
const busy = ref(false)
const error = ref('')
const message = ref('')
let mounted = false
let request: AbortController | null = null
let timer: ReturnType<typeof setTimeout> | undefined

function schedule() {
  clearTimeout(timer)
  if (mounted && !document.hidden) timer = setTimeout(() => void refresh(), 10000)
}

async function refresh() {
  if (!mounted || busy.value || document.hidden || request) return
  const controller = new AbortController()
  request = controller
  loading.value = true
  try {
    const current = await api.getGoogleTasksStatus(controller.signal)
    const unresolved = current.enabled ? await api.getGoogleTaskConflicts(controller.signal) : []
    const uncertain = await api.getGoogleTaskRecoveries(controller.signal)
    if (!mounted || controller.signal.aborted) return
    status.value = current
    recoveries.value = uncertain
    conflicts.value = unresolved
    queuedChoices.value = new Set()
    if (!selectedList.value) selectedList.value = current.task_list_id ?? ''
    if (current.timezone) timezone.value = current.timezone
    error.value = ''
    // Local recovery and disable controls remain available if Google is offline.
    const available = current.authorized ? await api.listGoogleTaskLists(controller.signal) : []
    if (mounted && !controller.signal.aborted) lists.value = available
  } catch (cause) {
    if (mounted && !controller.signal.aborted)
      error.value =
        cause instanceof Error ? cause.message : 'Could not refresh Google Tasks status.'
  } finally {
    if (request === controller) request = null
    if (mounted && !controller.signal.aborted) {
      loading.value = false
      schedule()
    }
  }
}

async function perform(action: () => Promise<void>) {
  if (busy.value || !status.value) return
  clearTimeout(timer)
  request?.abort()
  request = null
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    await action()
  } catch (cause) {
    if (mounted)
      error.value =
        cause instanceof Error
          ? cause.message
          : 'Request not confirmed. Refresh status before retrying.'
  } finally {
    if (mounted) {
      busy.value = false
      loading.value = false
      schedule()
    }
  }
}

async function save(enabled: boolean) {
  const current = status.value
  if (!current || (enabled && !selectedList.value)) return
  await perform(async () => {
    const saved = await api.configureGoogleTasks({
      enabled,
      expected_version: current.version,
      task_list_id: enabled ? selectedList.value : undefined,
      timezone: enabled ? timezone.value : undefined,
    })
    if (!mounted) return
    status.value = saved
    message.value = enabled
      ? 'Google Tasks sync enabled. Initial synchronization is queued.'
      : 'Google Tasks sync disabled. Both apps keep their tasks; Calendar sync is unchanged.'
  })
}

async function createList() {
  const current = status.value
  if (!current || current.list_create_attempted) return
  await perform(async () => {
    try {
      const created = await api.createGoogleTaskList(current.version)
      if (!mounted) return
      lists.value = [...lists.value, created]
      selectedList.value = created.id
      message.value = 'List created. Enable sync when ready.'
    } finally {
      // Even a failed request may have created the list. Refresh its durable
      // attempt flag before offering another create, never retry POST blindly.
      if (mounted) {
        status.value = { ...current, list_create_attempted: true }
        const refreshed = await api.getGoogleTasksStatus()
        if (mounted) status.value = refreshed
      }
    }
  })
}

async function leaveUnlinked(recovery: GoogleTaskRecovery) {
  if (
    !window.confirm(
      `Leave “${recovery.title}” unlinked? Both copies stay unchanged. This task will no longer sync, and no replacement will be created.`,
    )
  )
    return
  await perform(async () => {
    await api.leaveGoogleTaskUnlinked(recovery.link_id)
    if (!mounted) return
    recoveries.value = recoveries.value.filter((item) => item.link_id !== recovery.link_id)
    message.value = 'Task left unlinked. Both copies are kept; no replacement will be created.'
  })
  await nextTick()
  if (
    mounted &&
    document.activeElement === document.body &&
    !recoveries.value.some((item) => item.link_id === recovery.link_id)
  ) {
    refreshButton.value?.focus({ preventScroll: true })
  }
}

async function syncNow() {
  await perform(async () => {
    await api.syncGoogleTasks()
    if (mounted) message.value = 'Google Tasks synchronization queued; changes are not applied yet.'
  })
}

async function resolveConflict(
  conflict: GoogleTaskConflict,
  choice: GoogleTaskResolutionRequest['choice'],
) {
  await perform(async () => {
    await api.resolveGoogleTaskConflict(conflict.link_id, { conflict_id: conflict.id, choice })
    if (!mounted) return
    queuedChoices.value.add(conflict.id)
    message.value =
      'Conflict choice queued. Both copies will be checked again before changes are applied.'
  })
}

function visibilityChanged() {
  if (document.hidden) {
    clearTimeout(timer)
    request?.abort()
    request = null
  } else void refresh()
}

onMounted(() => {
  mounted = true
  document.addEventListener('visibilitychange', visibilityChanged)
  void refresh()
})
onBeforeUnmount(() => {
  mounted = false
  request?.abort()
  clearTimeout(timer)
  document.removeEventListener('visibilitychange', visibilityChanged)
})
</script>

<template>
  <section
    aria-labelledby="google-tasks-heading"
    class="border-b border-slate-200 py-8 dark:border-slate-800"
  >
    <h2 id="google-tasks-heading" class="text-sm font-semibold">Google Tasks</h2>
    <p class="mt-2 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
      Sync titles, deadline dates, and completion both ways. Precise deadline times stay in rakkio;
      scheduled work blocks still use Calendar. Deleting in either app keeps the other copy.
    </p>
    <p class="mt-2 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
      Google Tasks is optional. We use its data only for sync, not advertising or AI training.
      Enabling sync shares the fields above with your chosen list. See the
      <RouterLink class="underline underline-offset-4" to="/privacy">Privacy Policy</RouterLink>
      and <RouterLink class="underline underline-offset-4" to="/terms">Terms</RouterLink>.
    </p>
    <p v-if="error" role="alert" class="mt-3 text-sm text-red-600 dark:text-red-400">{{ error }}</p>
    <p v-if="message" role="status" class="mt-3 text-sm text-slate-600 dark:text-slate-300">
      {{ message }}
    </p>
    <div v-if="status" class="mt-4 space-y-4">
      <a
        v-if="!status.authorized"
        :href="connectUrl"
        class="inline-flex rounded-md bg-slate-950 px-4 py-2 text-sm font-medium text-white dark:bg-white dark:text-slate-950"
        >Connect Google Tasks</a
      >
      <button
        v-if="!status.authorized && status.enabled"
        type="button"
        :disabled="busy"
        class="rounded-md border border-slate-300 px-4 py-2 text-sm font-medium disabled:opacity-50 dark:border-slate-700"
        @click="save(false)"
      >
        Disable Tasks sync
      </button>
      <template v-if="status.authorized">
        <div class="flex max-w-lg flex-col items-stretch gap-3 sm:flex-row sm:items-end">
          <label class="min-w-0 flex-1 text-sm font-medium">
            Task list
            <select
              v-model="selectedList"
              :disabled="busy || status.enabled"
              class="mt-2 w-full rounded-md border border-slate-300 bg-white px-3 py-2 dark:border-slate-700 dark:bg-slate-950"
            >
              <option value="">Choose a list</option>
              <option v-for="list in lists" :key="list.id" :value="list.id">
                {{ list.title }}
              </option>
            </select>
          </label>
          <button
            v-if="!status.enabled"
            type="button"
            :disabled="busy || status.list_create_attempted || !!status.task_list_id"
            class="rounded-md border border-slate-300 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-slate-700"
            @click="createList"
          >
            Create rakkio list
          </button>
        </div>
        <p class="text-sm text-slate-500 dark:text-slate-400">
          Sync timezone: {{ timezone }}. Enabling sync copies your tasks to this list and imports
          tasks already in it.
        </p>
        <p
          v-if="status.list_create_attempted && !selectedList"
          class="text-sm text-slate-500 dark:text-slate-400"
        >
          An earlier list creation may have succeeded. Refresh lists and choose the existing list.
        </p>
        <div class="flex flex-wrap gap-3">
          <button
            v-if="!status.enabled"
            type="button"
            :disabled="busy || !selectedList"
            class="rounded-md bg-slate-950 px-4 py-2 text-sm font-medium text-white disabled:opacity-50 dark:bg-white dark:text-slate-950"
            @click="save(true)"
          >
            Enable Tasks sync
          </button>
          <template v-else>
            <button
              type="button"
              :disabled="busy"
              class="rounded-md bg-slate-950 px-4 py-2 text-sm font-medium text-white disabled:opacity-50 dark:bg-white dark:text-slate-950"
              @click="syncNow"
            >
              Sync Tasks now
            </button>
            <button
              type="button"
              :disabled="busy"
              class="rounded-md border border-slate-300 px-4 py-2 text-sm font-medium disabled:opacity-50 dark:border-slate-700"
              @click="save(false)"
            >
              Disable Tasks sync
            </button>
          </template>
        </div>
        <p v-if="status.last_synced_at" class="text-sm text-slate-500 dark:text-slate-400">
          Last Tasks sync:
          {{
            new Date(status.last_synced_at).toLocaleString(undefined, {
              dateStyle: 'medium',
              timeStyle: 'short',
            })
          }}
        </p>
        <p v-if="status.last_error" role="alert" class="text-sm text-red-600 dark:text-red-400">
          {{ status.last_error }}
        </p>
      </template>
    </div>
    <div v-if="recoveries.length" class="mt-6 space-y-4">
      <h3 class="text-sm font-semibold">Unconfirmed task creation</h3>
      <p class="max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
        Google may already have created these tasks. Check again to find their existing copies
        without creating duplicates. Or leave them unlinked to stop syncing them. Keep the rakkio
        reference in Google task notes so existing copies can still be recognized.
      </p>
      <button
        type="button"
        class="secondary-button"
        :disabled="busy || !status?.enabled || !status.authorized"
        @click="syncNow"
      >
        Check Google again
      </button>
      <article
        v-for="recovery in recoveries"
        :key="recovery.link_id"
        class="rounded-lg border border-slate-200 p-4 dark:border-slate-800"
      >
        <h4 class="break-words text-sm font-semibold">{{ recovery.title }}</h4>
        <button
          type="button"
          class="secondary-button mt-3"
          :disabled="busy"
          @click="leaveUnlinked(recovery)"
        >
          Leave unlinked
        </button>
      </article>
      <p v-if="recoveries.length === 100" class="text-sm text-slate-500 dark:text-slate-400">
        Showing the first 100 tasks. More will appear after these are handled.
      </p>
    </div>
    <div v-if="status?.enabled && conflicts.length" class="mt-6 space-y-4">
      <h3 class="text-sm font-semibold">Conflicting task edits</h3>
      <p class="text-sm text-slate-500 dark:text-slate-400">
        Choose which app to keep for conflicting fields only. Changes to other fields are preserved.
      </p>
      <article
        v-for="conflict in conflicts"
        :key="conflict.id"
        class="rounded-lg border border-slate-200 p-4 dark:border-slate-800"
      >
        <h4 class="break-words text-sm font-semibold">{{ conflict.local.title }}</h4>
        <dl class="mt-3 grid min-w-0 gap-3 text-sm sm:grid-cols-2">
          <div class="min-w-0">
            <dt class="font-medium">rakkio</dt>
            <dd class="mt-1 break-words">{{ conflict.local.title }}</dd>
            <dd>Deadline date: {{ conflict.local.date ?? 'None' }}</dd>
            <dd>{{ conflict.local.completed ? 'Completed' : 'Not completed' }}</dd>
          </div>
          <div class="min-w-0">
            <dt class="font-medium">Google Tasks</dt>
            <dd class="mt-1 break-words">{{ conflict.google.title }}</dd>
            <dd>Deadline date: {{ conflict.google.date ?? 'None' }}</dd>
            <dd>{{ conflict.google.completed ? 'Completed' : 'Not completed' }}</dd>
          </div>
        </dl>
        <div class="mt-4 flex flex-wrap gap-2">
          <button
            type="button"
            class="secondary-button"
            :disabled="busy || !status.authorized || queuedChoices.has(conflict.id)"
            @click="resolveConflict(conflict, 'prosepect')"
          >
            Keep rakkio edits
          </button>
          <button
            type="button"
            class="secondary-button"
            :disabled="busy || !status.authorized || queuedChoices.has(conflict.id)"
            @click="resolveConflict(conflict, 'google')"
          >
            Keep Google edits
          </button>
        </div>
        <p
          v-if="queuedChoices.has(conflict.id)"
          class="mt-2 text-sm text-slate-500 dark:text-slate-400"
        >
          Choice queued
        </p>
      </article>
      <p v-if="conflicts.length === 100" class="text-sm text-slate-500 dark:text-slate-400">
        Showing the first 100 conflicts. More will appear after these are resolved.
      </p>
    </div>
    <button
      ref="refreshButton"
      type="button"
      :disabled="busy"
      class="mt-4 text-sm font-medium underline underline-offset-4 disabled:opacity-50"
      @click="refresh"
    >
      {{ loading ? 'Refreshing Tasks status…' : 'Refresh Tasks status and lists' }}
    </button>
  </section>
</template>
