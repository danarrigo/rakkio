<script setup lang="ts">
import { onMounted, ref } from 'vue'
import * as api from '../api/client'
import type { EdunexStatus, EdunexSyncReport } from '../api/types'
import { useWorkspaceStore } from '../stores/workspace'

const store = useWorkspaceStore()
const status = ref<EdunexStatus | null>(null)
const token = ref('')
const loading = ref(true)
const busy = ref(false)
const error = ref('')
const message = ref('')

const dateTime = (value: string) =>
  new Date(value).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
const date = (value: string) =>
  new Date(value).toLocaleDateString(undefined, { dateStyle: 'medium' })

function describe(cause: unknown) {
  if (cause instanceof api.ApiError && cause.status === 502)
    return 'Edunex could not be reached. Try again later.'
  return cause instanceof Error ? cause.message : 'Something went wrong. Try again.'
}

function summarize(report: EdunexSyncReport) {
  const { created, updated, completed } = report.summary
  if (!created && !updated && !completed) return 'Synced. Everything was already up to date.'
  return `Synced. ${created} added, ${updated} updated, ${completed} marked done.`
}

async function load() {
  loading.value = true
  try {
    status.value = await api.getEdunexStatus()
    error.value = ''
  } catch (cause) {
    error.value = describe(cause)
  } finally {
    loading.value = false
  }
}

async function run(action: () => Promise<EdunexSyncReport>) {
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    const report = await action()
    status.value = report.status
    message.value = summarize(report)
    token.value = ''
    await store.refresh()
  } catch (cause) {
    const failure = describe(cause)
    await load()
    error.value = failure
  } finally {
    busy.value = false
  }
}

async function disconnect() {
  if (!window.confirm('Disconnect Edunex? Imported tasks stay in rakkio.')) return
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    await api.disconnectEdunex()
    message.value = 'Edunex disconnected. Imported tasks were kept.'
    await load()
  } catch (cause) {
    error.value = describe(cause)
  } finally {
    busy.value = false
  }
}

onMounted(load)
</script>

<template>
  <section
    class="border-b border-slate-200 py-8 dark:border-slate-800"
    aria-labelledby="edunex-title"
  >
    <div class="flex flex-wrap items-center gap-3">
      <h2 id="edunex-title" class="text-sm font-semibold">Edunex (ITB)</h2>
      <span
        class="rounded-full bg-slate-100 px-2 py-0.5 text-xs text-slate-500 dark:bg-slate-900 dark:text-slate-400"
        >Only visible to you</span
      >
    </div>
    <p class="mt-3 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
      Imports your Edunex assignments as tasks with their deadlines and the
      <code>edunex</code> label. Deadline changes on Edunex are copied, and submitting an assignment
      marks its task done. Tasks you delete here are not imported again. rakkio checks Edunex every
      few hours.
    </p>

    <p v-if="loading && !status" role="status" class="mt-4 text-sm text-slate-500">
      Checking connection…
    </p>
    <template v-else-if="status">
      <div v-if="status.connected" class="mt-4 space-y-1 text-sm">
        <p>
          Connected · {{ status.linked_tasks }}
          {{ status.linked_tasks === 1 ? 'assignment' : 'assignments' }} imported
        </p>
        <p class="text-slate-500 dark:text-slate-400">
          <span v-if="status.last_synced_at"
            >Last synced {{ dateTime(status.last_synced_at) }}</span
          >
          <span v-else>Not synced yet</span>
          <span v-if="status.token_expires_at">
            · Token expires {{ date(status.token_expires_at) }}</span
          >
        </p>
        <p v-if="status.last_error" role="alert" class="text-sm text-amber-700 dark:text-amber-400">
          {{ status.last_error }}
        </p>
      </div>

      <form
        class="mt-5 max-w-2xl"
        aria-label="Connect Edunex"
        @submit.prevent="run(() => api.connectEdunex(token))"
      >
        <label class="field-label" for="edunex-token">
          {{ status.connected ? 'Replace token' : 'Edunex token' }}
        </label>
        <div class="mt-2 flex flex-wrap gap-2">
          <input
            id="edunex-token"
            v-model="token"
            class="field-input min-w-0 flex-1"
            type="password"
            autocomplete="off"
            spellcheck="false"
            placeholder="Paste the Authorization value"
            :disabled="busy"
          />
          <button class="primary-button" type="submit" :disabled="busy || !token.trim()">
            {{ busy ? 'Connecting…' : status.connected ? 'Replace and sync' : 'Connect and sync' }}
          </button>
        </div>
        <details class="mt-3 text-sm leading-6 text-slate-500 dark:text-slate-400">
          <summary class="cursor-pointer font-medium text-slate-600 dark:text-slate-300">
            How to find the token
          </summary>
          <ol class="mt-2 list-decimal space-y-1 pl-5">
            <li>Sign in to Edunex and open the Assignments page.</li>
            <li>Open developer tools (F12) and choose the Network tab, then reload the page.</li>
            <li>Select a request to <code>api-edunex.cognisia.id</code>.</li>
            <li>
              Under request headers, copy the <code>Authorization</code> value. The
              <code>Bearer</code> prefix is optional.
            </li>
          </ol>
          <p class="mt-2">
            The token acts as your Edunex login, so treat it like a password. rakkio stores it
            encrypted and sends it only to Edunex.
          </p>
        </details>
      </form>

      <div v-if="status.connected" class="mt-5 flex flex-wrap gap-2">
        <button
          class="secondary-button"
          type="button"
          :disabled="busy"
          @click="run(api.synchronizeEdunex)"
        >
          {{ busy ? 'Syncing…' : 'Sync now' }}
        </button>
        <button class="secondary-button" type="button" :disabled="busy" @click="disconnect">
          Disconnect
        </button>
      </div>
    </template>

    <p v-if="message" role="status" class="mt-4 text-sm text-slate-500 dark:text-slate-400">
      {{ message }}
    </p>
    <p v-if="error" role="alert" class="mt-4 text-sm text-rose-600">{{ error }}</p>
  </section>
</template>
