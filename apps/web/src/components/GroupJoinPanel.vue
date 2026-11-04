<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import * as api from '../api/client'
import type { GroupInviteReference } from '../api/types'
import { useGroupsStore, GROUP_ROSTER_NOTICE_VERSION } from '../stores/groups'
import { useWorkspaceStore } from '../stores/workspace'

const props = defineProps<{ reference?: GroupInviteReference | null }>()
const groups = useGroupsStore()
const workspace = useWorkspaceStore()
const input = ref(groups.pendingCredential || '')
const preview = ref<Awaited<ReturnType<typeof api.previewGroupInvitation>> | null>(null)
const resolvedReference = ref<GroupInviteReference | null>(null)
const handle = ref<string | null>(null)
const acknowledged = ref(false)
const loading = ref(false)
const error = ref('')
const message = ref('')
let controller: AbortController | undefined
let timer: ReturnType<typeof setTimeout> | undefined
let generation = 0
let alive = true
function cancel() {
  generation++
  controller?.abort()
  clearTimeout(timer)
}
function reset() {
  cancel()
  preview.value = null
  resolvedReference.value = null
  acknowledged.value = false
  loading.value = false
  error.value = ''
  message.value = ''
}
function enteredReference(): GroupInviteReference | null {
  let value = input.value.trim()
  if (value.startsWith('https://') || value.startsWith('http://')) {
    try {
      const url = new URL(value)
      if (url.origin !== window.location.origin || url.pathname !== '/join' || url.search)
        return null
      value = new URLSearchParams(url.hash.slice(1)).get('invite') || ''
    } catch {
      return null
    }
  }
  if (/^[a-zA-Z0-9_-]{43}$/.test(value)) return { credential: value }
  const code = value.replace(/[ -]/g, '').toUpperCase()
  return /^[0-9A-HJKMNP-TV-Z]{12}$/.test(code) ? { credential: code } : null
}
async function review(value?: GroupInviteReference) {
  if (!groups.enabled || !workspace.user || groups.busy) return
  const selected = value || enteredReference()
  reset()
  if (!selected) {
    error.value = 'Enter a valid join code or a rakkio invitation link.'
    return
  }
  const owner = workspace.user.id
  const version = generation
  const request = new AbortController()
  controller = request
  timer = setTimeout(() => request.abort(), 15000)
  const current = () => alive && version === generation && workspace.user?.id === owner
  loading.value = true
  try {
    const [result, own] = await Promise.all([
      api.previewGroupInvitation(selected, request.signal),
      api.getHandle(request.signal),
    ])
    if (!current()) return
    preview.value = result
    resolvedReference.value = selected
    handle.value = own.handle || null
  } catch (cause) {
    if (current())
      error.value = cause instanceof Error ? cause.message : 'Invitation could not be checked.'
  } finally {
    if (current()) {
      loading.value = false
      clearTimeout(timer)
    }
  }
}
async function join() {
  if (
    !preview.value ||
    !resolvedReference.value ||
    !acknowledged.value ||
    !handle.value ||
    !groups.canWrite
  )
    return
  const selected = resolvedReference.value
  const scope = generation
  const account = workspace.user?.id
  const name = preview.value.group_name
  const result = await groups.write((owner, signal) =>
    api.joinGroup(
      {
        ...selected,
        expected_user_id: owner,
        confirm_roster_visibility: true,
        roster_notice_version: GROUP_ROSTER_NOTICE_VERSION,
      },
      signal,
    ),
  )
  if (!result || !alive || scope !== generation || workspace.user?.id !== account) return
  groups.forgetInvitation()
  reset()
  input.value = ''
  message.value =
    result.status === 'pending'
      ? `Your request to join ${name} is awaiting leader approval.`
      : `You joined ${name}.`
}
watch(input, () => reset(), { flush: 'sync' })
watch(
  () => props.reference,
  (value) => {
    reset()
    if (value) void review(value)
  },
  { immediate: true },
)
watch(
  () => workspace.user?.id,
  () => {
    reset()
    handle.value = null
    input.value = groups.pendingCredential || ''
  },
)
onBeforeUnmount(() => {
  alive = false
  cancel()
})
</script>

<template>
  <section class="border border-slate-200 p-5 dark:border-slate-800" aria-labelledby="join-heading">
    <h2 id="join-heading" class="text-sm font-semibold">Join a group</h2>
    <form class="mt-4" aria-label="Review group invitation" @submit.prevent="review()">
      <label for="group-join-credential" class="field-label">Join code or invitation link</label>
      <input
        id="group-join-credential"
        v-model="input"
        class="field-input mt-2"
        autocomplete="off"
        :spellcheck="false"
        maxlength="2048"
        :disabled="loading || groups.busy"
      />
      <button
        class="secondary-button mt-3"
        type="submit"
        :disabled="!input.trim() || loading || groups.busy"
      >
        {{ loading ? 'Checking…' : 'Review invitation' }}
      </button>
    </form>
    <p v-if="error" role="alert" class="mt-3 text-sm text-rose-700 dark:text-rose-400">
      {{ error }}
    </p>
    <p v-if="message" role="status" class="mt-3 text-sm text-emerald-700 dark:text-emerald-400">
      {{ message }}
    </p>
    <div v-if="preview" class="mt-5 border-t border-slate-200 pt-4 dark:border-slate-800">
      <h3 class="break-words font-medium">{{ preview.group_name }}</h3>
      <p v-if="handle" class="mt-2 text-sm">
        Joining as <strong>@{{ handle }}</strong
        >.
      </p>
      <p v-else class="mt-2 text-sm">
        Choose your @handle in
        <RouterLink to="/settings" class="underline">Settings</RouterLink> before joining. Keep your
        original invitation so you can return here.
      </p>
      <p class="mt-2 text-sm leading-6 text-slate-500 dark:text-slate-400">
        {{
          preview.approval_required
            ? 'A leader must approve your request.'
            : 'Confirming will add you to this group.'
        }}
        Reviewing an invitation never joins automatically.
      </p>
      <label class="mt-4 flex items-start gap-3 text-sm leading-6"
        ><input
          v-model="acknowledged"
          type="checkbox"
          class="mt-1.5 shrink-0"
          :disabled="!handle || groups.busy"
        /><span
          >I agree that group members can see my display name, @handle and role. My personal content
          stays private; assignment progress is private unless I choose to share it.</span
        ></label
      >
      <button
        class="primary-button mt-4"
        type="button"
        :disabled="!acknowledged || !handle || !groups.canWrite"
        @click="join"
      >
        {{ preview.approval_required ? 'Request to join' : 'Confirm joining' }}
      </button>
    </div>
  </section>
</template>
