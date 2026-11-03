<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import * as api from '../api/client'
import type { HandleProfile } from '../api/types'
import { useWorkspaceStore } from '../stores/workspace'

const store = useWorkspaceStore()
const profile = ref<HandleProfile | null>(null)
const input = ref('')
const acknowledged = ref(false)
const loading = ref(false)
const busy = ref(false)
const stale = ref(true)
const error = ref('')
const message = ref('')
const entered = computed(() => input.value.trim().replace(/^@/, ''))
const valid = computed(() => /^[a-zA-Z0-9_]{3,30}$/.test(entered.value))
const canonical = computed(() => entered.value.toLowerCase())
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
  const owner = store.user?.id
  const request = new AbortController()
  controller = request
  timeout = setTimeout(() => request.abort(), 15000)
  return {
    signal: request.signal,
    current: () => alive && version === generation && owner === store.user?.id,
  }
}
async function refresh() {
  if (!store.user || busy.value) return
  const request = begin()
  acknowledged.value = false
  loading.value = true
  error.value = ''
  try {
    const result = await api.getHandle(request.signal)
    if (!request.current()) return
    profile.value = result
    stale.value = false
  } catch (cause) {
    if (request.current()) {
      stale.value = true
      error.value =
        cause instanceof Error
          ? `${cause.message} Refresh before claiming.`
          : 'Could not load your handle. Refresh before claiming.'
    }
  } finally {
    if (request.current()) {
      loading.value = false
      clearTimeout(timeout)
    }
  }
}
async function claim() {
  if (
    !store.user ||
    profile.value?.handle ||
    !profile.value ||
    stale.value ||
    loading.value ||
    busy.value ||
    !valid.value ||
    !acknowledged.value
  )
    return
  const requested = canonical.value
  const request = begin()
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    const result = await api.claimHandle(
      { handle: requested, expected_user_id: store.user.id },
      request.signal,
    )
    if (!request.current()) return
    if (result.handle !== requested) throw new Error('The handle claim was not confirmed.')
    profile.value = result
    input.value = ''
    acknowledged.value = false
    message.value = 'Your handle has been claimed.'
  } catch (cause) {
    if (request.current()) {
      stale.value = true
      acknowledged.value = false
      error.value =
        cause instanceof Error
          ? `${cause.message} Refresh to check the result before trying again.`
          : 'Claim not confirmed. Refresh to check the result before trying again.'
    }
  } finally {
    if (request.current()) {
      busy.value = false
      clearTimeout(timeout)
    }
  }
}
watch(input, () => {
  acknowledged.value = false
})
watch(
  () => store.user?.id,
  () => {
    cancel()
    profile.value = null
    input.value = ''
    acknowledged.value = false
    loading.value = false
    busy.value = false
    stale.value = true
    error.value = ''
    message.value = ''
    void refresh()
  },
  { immediate: true },
)
onBeforeUnmount(() => {
  alive = false
  cancel()
})
</script>

<template>
  <section
    class="border-b border-slate-200 py-8 dark:border-slate-800"
    aria-labelledby="handle-heading"
    :aria-busy="loading || busy"
  >
    <div class="flex flex-wrap items-center justify-between gap-3">
      <h2 id="handle-heading" class="text-sm font-semibold">Your @handle</h2>
      <button type="button" class="secondary-button" :disabled="loading || busy" @click="refresh">
        Refresh handle
      </button>
    </div>
    <p class="mt-3 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
      A unique public name for group invitations as Groups becomes available. Your display name,
      email and Discord account are unchanged. There is no public account directory.
    </p>
    <p v-if="loading && !profile" role="status" class="mt-4 text-sm">Loading your handle…</p>
    <p v-if="error" role="alert" class="mt-4 text-sm text-rose-700 dark:text-rose-400">
      {{ error }}
    </p>
    <p v-if="message" role="status" class="mt-4 text-sm text-emerald-700 dark:text-emerald-400">
      {{ message }}
    </p>
    <div v-if="profile?.handle" class="mt-4">
      <p class="break-all font-medium">@{{ profile.handle }}</p>
      <p class="mt-2 text-sm leading-6 text-slate-500 dark:text-slate-400">
        This handle cannot be changed or transferred to another account.
      </p>
    </div>
    <form
      v-else-if="profile"
      class="mt-5 max-w-2xl"
      aria-label="Claim your handle"
      @submit.prevent="claim"
    >
      <label for="profile-handle" class="field-label">Handle</label>
      <input
        id="profile-handle"
        v-model="input"
        class="field-input mt-2"
        name="handle"
        maxlength="64"
        autocomplete="off"
        autocapitalize="none"
        :spellcheck="false"
        aria-describedby="handle-help"
        :disabled="busy || loading || stale"
        placeholder="@yourname"
      />
      <p id="handle-help" class="mt-2 text-sm leading-6 text-slate-500 dark:text-slate-400">
        3-30 letters, numbers or underscores. Capitalization does not matter. Some names are
        reserved.
      </p>
      <p v-if="valid" class="mt-3 break-all text-sm">
        You are choosing <strong>@{{ canonical }}</strong
        >.
      </p>
      <label class="mt-4 flex items-start gap-3 text-sm leading-6">
        <input
          v-model="acknowledged"
          type="checkbox"
          class="mt-1.5 shrink-0"
          :disabled="busy || loading || stale"
        />
        <span
          >I understand this handle is public and permanent. It cannot be changed or reused after
          account deletion; only a one-way reservation remains to prevent impersonation.</span
        >
      </label>
      <p class="mt-3 text-sm">
        <RouterLink to="/privacy#handles" class="underline underline-offset-4"
          >Handle privacy and retention</RouterLink
        >
      </p>
      <button
        class="primary-button mt-4"
        type="submit"
        :disabled="!valid || !acknowledged || busy || loading || stale"
      >
        {{ busy ? 'Claiming…' : 'Claim handle' }}
      </button>
    </form>
  </section>
</template>
