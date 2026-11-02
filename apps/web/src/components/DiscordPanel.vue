<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import * as api from '../api/client'
import type { DiscordStatus } from '../api/types'
import { useWorkspaceStore } from '../stores/workspace'

const store = useWorkspaceStore()
const status = ref<DiscordStatus | null>(null)
const consent = ref(false)
const timezone = ref(Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC')
const loading = ref(true)
const busy = ref(false)
const stale = ref(true)
const error = ref('')
const message = ref('')
let active = false
let generation = 0
let controller: AbortController | undefined
let timeout: ReturnType<typeof setTimeout> | undefined

function discordUrl(value: string | null | undefined) {
  if (!value) return undefined
  try {
    const url = new URL(value)
    if (url.origin === 'https://discord.com' && url.pathname === '/oauth2/authorize')
      return url.href
  } catch {
    /* An untrusted authorization URL must never become a link/navigation. */
  }
  return undefined
}
const installUrl = computed(() => discordUrl(status.value?.install_url))

function cancel() {
  generation++
  controller?.abort()
  clearTimeout(timeout)
}
function begin() {
  cancel()
  const request = new AbortController()
  controller = request
  const current = generation
  const owner = store.user?.id
  timeout = setTimeout(() => request.abort(), 15000)
  return {
    signal: request.signal,
    current: () => active && current === generation && owner === store.user?.id,
  }
}

async function refresh() {
  if (!active || busy.value || !store.user) return
  const request = begin()
  loading.value = true
  try {
    const result = await api.getDiscordStatus(request.signal)
    if (!request.current()) return
    status.value = result
    if (result.connection) timezone.value = result.connection.timezone
    stale.value = false
    error.value = ''
    if (new URL(window.location.href).searchParams.get('discord') === 'cancelled')
      message.value = 'Discord connection cancelled. No new connection was added.'
  } catch (cause) {
    if (request.current()) {
      stale.value = true
      error.value =
        cause instanceof Error
          ? `${cause.message} Refresh status before making changes.`
          : 'Could not load Discord status. Refresh before making changes.'
    }
  } finally {
    if (request.current()) {
      loading.value = false
      clearTimeout(timeout)
    }
  }
}

async function connect() {
  if (!status.value?.configured || !consent.value || busy.value || stale.value) return
  const request = begin()
  busy.value = true
  error.value = ''
  try {
    const result = await api.connectDiscord(
      { timezone: timezone.value, consent_version: status.value.consent_version },
      request.signal,
    )
    if (!request.current()) return
    const url = discordUrl(result.authorization_url)
    if (!url) throw new Error('Discord returned an invalid authorization URL.')
    window.location.assign(url)
  } catch (cause) {
    if (request.current()) {
      stale.value = true
      error.value =
        cause instanceof Error
          ? `${cause.message} Refresh status before retrying.`
          : 'Connection not confirmed. Refresh status before retrying.'
    }
  } finally {
    if (request.current()) {
      busy.value = false
      clearTimeout(timeout)
    }
  }
}

async function disconnect() {
  const connection = status.value?.connection
  if (
    !connection ||
    busy.value ||
    stale.value ||
    !window.confirm(
      'Disconnect Discord? Future commands will stop working. Your tasks and messages already sent to Discord will remain.',
    )
  )
    return
  const request = begin()
  busy.value = true
  error.value = ''
  try {
    await api.disconnectDiscord({ expected_id: connection.id }, request.signal)
    if (!request.current()) return
    status.value = status.value ? { ...status.value, connection: null } : null
    consent.value = false
    message.value = 'Discord disconnected. Your tasks are unchanged.'
  } catch (cause) {
    if (request.current()) {
      stale.value = true
      error.value =
        cause instanceof Error
          ? `${cause.message} Refresh status before retrying.`
          : 'Disconnection not confirmed. Refresh status before retrying.'
    }
  } finally {
    if (request.current()) {
      busy.value = false
      clearTimeout(timeout)
    }
  }
}

watch(
  () => store.user?.id,
  () => {
    cancel()
    status.value = null
    consent.value = false
    timezone.value = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
    error.value = ''
    message.value = ''
    busy.value = false
    stale.value = true
    void refresh()
  },
  { flush: 'sync' },
)
onMounted(() => {
  active = true
  void refresh()
})
onBeforeUnmount(() => {
  active = false
  cancel()
})
</script>

<template>
  <section
    class="border-b border-slate-200 py-8 dark:border-slate-800"
    aria-labelledby="discord-title"
  >
    <div class="flex flex-wrap items-center justify-between gap-3">
      <h2 id="discord-title" class="text-sm font-semibold">Discord</h2>
      <button class="secondary-button" type="button" :disabled="loading || busy" @click="refresh">
        {{ loading ? 'Loading Discord…' : 'Refresh Discord status' }}
      </button>
    </div>
    <p class="mt-3 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
      Capture tasks and check your plan without leaving Discord. Replies are visible only to you,
      not the channel. No chat history access, AI, or automatic messages.
    </p>
    <p v-if="loading && !status" role="status" class="mt-4 text-sm text-slate-500">
      Checking connection…
    </p>
    <template v-else-if="status">
      <template v-if="status.connection">
        <p class="mt-4 break-words text-sm">
          Connected as <strong class="font-medium">{{ status.connection.username }}</strong>
          <span class="text-slate-500 dark:text-slate-400">
            · {{ status.connection.timezone }}</span
          >
        </p>
        <p class="mt-3 text-sm leading-6 text-slate-500 dark:text-slate-400">
          <code>/add title:Finish report due:tomorrow</code> adds a task to Inbox.
          <code>/today</code> shows your plan. <code>/done</code> lets you choose a task to
          complete.
        </p>
        <p v-if="!status.configured" class="mt-3 text-sm text-slate-500 dark:text-slate-400">
          Discord is disabled on this installation. You can still disconnect the saved account link.
        </p>
        <div class="mt-5 flex flex-wrap gap-3">
          <a
            v-if="status.configured && installUrl"
            class="secondary-button"
            :href="installUrl"
            target="_blank"
            rel="noopener noreferrer"
            >Install Discord app</a
          >
          <button
            class="secondary-button"
            type="button"
            :disabled="busy || stale || loading"
            @click="disconnect"
          >
            {{ busy ? 'Working…' : 'Disconnect Discord' }}
          </button>
        </div>
      </template>
      <form
        v-else-if="status.configured"
        class="mt-5 max-w-2xl"
        aria-label="Connect Discord"
        @submit.prevent="connect"
      >
        <label class="block max-w-sm">
          <span class="field-label">Discord command timezone</span>
          <input
            v-model="timezone"
            class="field-input"
            maxlength="128"
            required
            :disabled="busy || stale"
            placeholder="Asia/Jakarta"
            autocomplete="off"
          />
        </label>
        <label class="mt-5 flex items-start gap-3 text-sm leading-6">
          <input
            v-model="consent"
            class="mt-1.5 shrink-0"
            type="checkbox"
            :disabled="busy || stale"
          />
          <span>
            I agree to share task titles, deadlines, selections and completion actions requested
            through commands with Discord. This can include tasks synchronized with Google Tasks or
            Calendar. rakkio stores my Discord identity and command timezone; Discord processes
            command content under its own privacy policy. Disconnecting does not remove messages
            already sent.
          </span>
        </label>
        <div class="mt-5 flex flex-wrap items-center gap-4">
          <button
            class="primary-button"
            type="submit"
            :disabled="!consent || !timezone.trim() || busy || stale || loading"
          >
            {{ busy ? 'Connecting…' : 'Connect Discord' }}
          </button>
          <RouterLink class="text-sm underline underline-offset-4" to="/privacy"
            >Privacy Policy</RouterLink
          >
          <a
            class="text-sm underline underline-offset-4"
            href="https://discord.com/privacy"
            target="_blank"
            rel="noopener noreferrer"
            >Discord privacy</a
          >
        </div>
      </form>
      <p v-else class="mt-4 text-sm text-slate-500 dark:text-slate-400">
        Discord is not enabled on this installation.
      </p>
    </template>
    <p v-if="error" role="alert" class="mt-4 text-sm text-rose-600 dark:text-rose-400">
      {{ error }}
    </p>
    <p v-if="message" role="status" class="mt-4 text-sm text-slate-600 dark:text-slate-300">
      {{ message }}
    </p>
  </section>
</template>
