<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { onBeforeRouteLeave, onBeforeRouteUpdate, RouterLink, useRoute } from 'vue-router'
import * as api from '../api/client'
import type { GroupAssignment, GroupAssignmentUndo } from '../api/types'
import SafeMarkdown from '../components/SafeMarkdown.vue'
import { GROUP_PROGRESS_NOTICE_VERSION, useGroupAssignments } from '../stores/group-assignments'
import { useWorkspaceStore } from '../stores/workspace'

const route = useRoute(),
  workspace = useWorkspaceStore()
const groupId = computed(() => String(route.params.groupId || ''))
const assignments = useGroupAssignments(groupId)
const {
  group,
  items,
  preferences,
  usage,
  undos,
  next,
  nextUndo,
  state: lifecycle,
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
} = assignments
const acknowledgement = ref(false)
const titleInput = ref<HTMLInputElement | null>(null)
interface Draft {
  id: string | null
  actor: string
  membership: string
  version: number
  title: string
  description: string
  date: string
  time: string
  timezone: string
  state: 'open' | 'closed'
}
const draft = ref<Draft | null>(null)
const draftScope = computed(
  () =>
    !!draft.value &&
    draft.value.actor === workspace.user?.id &&
    draft.value.membership === group.value?.my_membership_id,
)
const canSave = computed(
  () =>
    canWrite.value &&
    leader.value &&
    draftScope.value &&
    !!draft.value?.title.trim() &&
    (!draft.value.id ||
      (latest.value?.id === draft.value.id && latest.value.version === draft.value.version)),
)
const now = ref(Date.now())
const clock = setInterval(() => {
  now.value = Date.now()
}, 1000)
function dismissDraft() {
  draft.value = null
  assignments.edit(null)
}
async function beginDraft(row?: GroupAssignment) {
  if (!canWrite.value || !leader.value || draft.value || !workspace.user || !group.value) return
  assignments.edit(row || null)
  draft.value = {
    id: row?.id || null,
    actor: workspace.user.id,
    membership: group.value.my_membership_id,
    version: row?.version || 0,
    title: row?.title || '',
    description: row?.description || '',
    date: row?.due_on || '',
    time: row?.due_time || '23:59',
    timezone: row?.timezone || group.value.timezone,
    state: row?.state === 'closed' ? 'closed' : 'open',
  }
  await nextTick()
  titleInput.value?.focus()
}
async function save() {
  if (!canSave.value || !draft.value) return
  const value = { ...draft.value }
  const result = await assignments.write(
    (actor, snapshot, signal) => {
      const fields = {
        expected_user_id: actor,
        title: value.title,
        description: value.description,
        due_on: value.date || null,
        due_time: value.date ? value.time || null : null,
        deadline_timezone: value.timezone,
      }
      return value.id
        ? api.updateGroupAssignment(
            snapshot.id,
            value.id,
            { ...fields, expected_version: value.version, state: value.state },
            signal,
          )
        : api.createGroupAssignment(snapshot.id, fields, signal)
    },
    value.id ? 'Assignment saved. Member completion is preserved.' : 'Shared assignment published.',
  )
  if (result) dismissDraft()
}
function adoptVersion() {
  if (!draft.value?.id || !latest.value || !canWrite.value || !leader.value || !draftScope.value)
    return
  if (
    window.confirm(
      'Use the latest assignment version with your draft? Review the current definition below first. Your draft values will replace it without resetting member completion.',
    )
  )
    draft.value.version = latest.value.version
}
async function complete(row: GroupAssignment) {
  await assignments.write(
    (actor, snapshot, signal) =>
      api.updateGroupAssignmentProgress(
        snapshot.id,
        row.id,
        {
          expected_user_id: actor,
          expected_membership_id: snapshot.my_membership_id,
          expected_assignment_version: row.version,
          expected_progress_version: row.my_progress.version,
          completed: !row.my_progress.completed_at,
        },
        signal,
      ),
    row.my_progress.completed_at ? 'Your work reopened.' : 'Your work marked complete.',
  )
}
async function share(enabled: boolean) {
  if (!preferences.value || (enabled && !acknowledgement.value)) return
  const version = preferences.value.version
  acknowledgement.value = false
  await assignments.write(
    (actor, snapshot, signal) =>
      api.updateGroupProgressPreferences(
        snapshot.id,
        {
          expected_user_id: actor,
          expected_membership_id: snapshot.my_membership_id,
          expected_version: version,
          share_progress: enabled,
          notice_version: enabled ? GROUP_PROGRESS_NOTICE_VERSION : null,
        },
        signal,
      ),
    enabled ? 'Progress sharing enabled for this group.' : 'Progress sharing stopped.',
  )
}
async function remove(row: GroupAssignment) {
  if (
    !canWrite.value ||
    !leader.value ||
    !window.confirm(
      `Delete “${row.title}” for everyone in this group? It will disappear immediately. Undo lasts 60 seconds and requires unchanged membership. Member completion is not erased until purge.`,
    )
  )
    return
  await assignments.write(
    (actor, snapshot, signal) =>
      api.deleteGroupAssignment(
        snapshot.id,
        row.id,
        { expected_user_id: actor, expected_version: row.version },
        signal,
      ),
    'Assignment deleted. Undo is available for 60 seconds.',
  )
}
async function restore(receipt: GroupAssignmentUndo) {
  if (Date.parse(receipt.undo_until) <= Date.now() || !leader.value) return
  await assignments.write(
    (actor, snapshot, signal) =>
      api.restoreGroupAssignment(
        snapshot.id,
        receipt.id,
        { expected_user_id: actor, expected_version: receipt.version },
        signal,
      ),
    'Assignment restored.',
  )
}
function deadline(row: GroupAssignment) {
  if (!row.due_on) return 'No deadline'
  const date = new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    timeZone: 'UTC',
  }).format(new Date(`${row.due_on}T00:00:00Z`))
  const time =
    row.due_time?.length === 8 && row.due_time.endsWith(':00')
      ? row.due_time.slice(0, 5)
      : row.due_time || '23:59'
  return `Due ${date} · ${time} · ${row.timezone}`
}
function leave() {
  if (busy.value)
    return window.confirm(
      'A change is still being confirmed. Leaving may not cancel it. Leave this page?',
    )
  return (
    !draft.value || window.confirm('Discard your unsaved assignment draft and leave this page?')
  )
}
function beforeUnload(event: BeforeUnloadEvent) {
  if (draft.value || busy.value) {
    event.preventDefault()
    event.returnValue = ''
  }
}
window.addEventListener('beforeunload', beforeUnload)
onBeforeRouteLeave(leave)
onBeforeRouteUpdate(leave)
watch(
  [() => workspace.user?.id, groupId],
  () => {
    dismissDraft()
    acknowledgement.value = false
  },
  { flush: 'sync' },
)
watch(
  [loading, busy],
  () => {
    acknowledgement.value = false
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  clearInterval(clock)
  window.removeEventListener('beforeunload', beforeUnload)
})
</script>

<template>
  <div class="mx-auto min-w-0 max-w-6xl space-y-6 px-5 py-10 sm:px-8 lg:px-12 lg:py-14">
    <header class="flex flex-wrap items-start justify-between gap-4">
      <div class="min-w-0">
        <RouterLink
          :to="{ name: 'groups' }"
          class="text-sm font-medium text-slate-600 underline underline-offset-4 dark:text-slate-300"
          >Back to Groups</RouterLink
        >
        <h1 class="mt-3 text-2xl font-semibold tracking-tight">Assignments</h1>
        <p v-if="group" class="mt-1 break-words text-sm text-slate-600 dark:text-slate-400">
          {{ group.name }} · {{ group.my_role }}
        </p>
      </div>
      <div class="flex flex-wrap gap-2">
        <button
          class="secondary-button"
          type="button"
          :disabled="loading || busy"
          @click="assignments.refresh"
        >
          Refresh
        </button>
        <button
          v-if="leader"
          class="primary-button"
          type="button"
          :disabled="!canWrite || !!draft"
          @click="beginDraft()"
        >
          New assignment
        </button>
      </div>
    </header>
    <p
      v-if="error"
      role="alert"
      class="rounded-md border border-rose-300 bg-rose-50 p-4 text-sm text-rose-800 dark:border-rose-900 dark:bg-rose-950/40 dark:text-rose-200"
    >
      {{ error }}
    </p>
    <p v-if="message" role="status" class="text-sm text-emerald-800 dark:text-emerald-300">
      {{ message }}
    </p>
    <p v-if="loading" role="status" class="text-sm text-slate-600 dark:text-slate-400">
      Loading group assignments…
    </p>
    <p v-if="!enabled && !loading && !error" class="text-sm">
      Groups are not available on this server.
    </p>
    <p v-if="stale && group && !loading" class="text-sm text-amber-800 dark:text-amber-300">
      Showing the last loaded values. Refresh before making more changes.
    </p>

    <form
      v-if="draft"
      class="min-w-0 rounded-md border border-slate-300 bg-white p-5 dark:border-slate-700 dark:bg-slate-950"
      aria-label="Assignment draft"
      @submit.prevent="save"
    >
      <h2 class="font-semibold">
        {{ draft.id ? 'Edit shared assignment' : 'New shared assignment' }}
      </h2>
      <p class="mt-2 text-sm text-slate-600 dark:text-slate-400">
        This definition is shared with the group. Completion stays individual.
      </p>
      <p v-if="!draftScope || !leader" class="mt-3 text-sm text-amber-800 dark:text-amber-300">
        Your group access or membership changed. Your draft is retained for copying, but cannot be
        saved.
      </p>
      <div
        v-if="draft.id && latest && latest.version !== draft.version"
        class="mt-4 space-y-3 rounded-md border border-amber-300 p-3 text-sm dark:border-amber-800"
      >
        <p class="font-medium">
          The assignment changed. Review the current definition before adopting its version.
        </p>
        <p class="break-words">{{ latest.title }}</p>
        <div class="max-h-48 overflow-auto">
          <SafeMarkdown
            :source="latest.description"
            formatting-only
            class="min-w-0 break-words [&_pre]:max-w-full [&_pre]:overflow-x-auto [&_table]:block [&_table]:max-w-full [&_table]:overflow-x-auto"
          />
        </div>
        <p>{{ deadline(latest) }} · {{ latest.state }}</p>
        <button
          type="button"
          class="secondary-button"
          :disabled="!canWrite || !draftScope || !leader"
          @click="adoptVersion"
        >
          Use latest version with my draft
        </button>
      </div>
      <p
        v-else-if="draft.id && !latest && !loading"
        class="mt-3 text-sm text-amber-800 dark:text-amber-300"
      >
        The assignment is no longer available. Your draft is retained for copying.
      </p>
      <label class="field-label mt-5" for="assignment-title">Assignment title</label>
      <input
        id="assignment-title"
        ref="titleInput"
        v-model="draft.title"
        class="field-input mt-2"
        maxlength="240"
        required
        :disabled="busy"
      />
      <label class="field-label mt-4" for="assignment-description">Description</label>
      <textarea
        id="assignment-description"
        v-model="draft.description"
        class="field-input mt-2 min-h-28"
        maxlength="10000"
        rows="4"
        :disabled="busy"
      />
      <div class="mt-4 grid min-w-0 gap-4 sm:grid-cols-2">
        <div class="min-w-0">
          <label class="field-label" for="assignment-date">Deadline date</label
          ><input
            id="assignment-date"
            v-model="draft.date"
            type="date"
            class="field-input mt-2 min-w-0"
            min="1900-01-01"
            max="9999-12-31"
            :disabled="busy"
          />
        </div>
        <div class="min-w-0">
          <label class="field-label" for="assignment-time">Deadline time</label
          ><input
            id="assignment-time"
            v-model="draft.time"
            type="time"
            :step="draft.time.length > 5 && draft.time.slice(6) !== '00' ? 1 : 60"
            class="field-input mt-2 min-w-0"
            :disabled="busy || !draft.date"
          />
        </div>
      </div>
      <label class="field-label mt-4" for="assignment-timezone">Deadline timezone (IANA)</label
      ><input
        id="assignment-timezone"
        v-model="draft.timezone"
        class="field-input mt-2"
        maxlength="64"
        required
        :disabled="busy"
      />
      <p class="mt-2 text-xs leading-5 text-slate-600 dark:text-slate-400">
        Date-only deadlines use 23:59. Shared deadlines do not reserve personal work time.
      </p>
      <template v-if="draft.id"
        ><label class="field-label mt-4" for="assignment-state">Assignment lifecycle</label
        ><select
          id="assignment-state"
          v-model="draft.state"
          class="field-input mt-2"
          :disabled="busy"
        >
          <option value="open">Open</option>
          <option value="closed">Closed history</option>
        </select>
        <p class="mt-2 text-xs leading-5 text-slate-600 dark:text-slate-400">
          Closing removes active work without completing members. Edits preserve their completion.
        </p></template
      >
      <div class="mt-5 flex flex-wrap gap-2">
        <button class="primary-button" type="submit" :disabled="!canSave">
          {{ draft.id ? 'Save assignment' : 'Publish assignment' }}</button
        ><button class="secondary-button" type="button" :disabled="busy" @click="dismissDraft">
          Discard draft
        </button>
      </div>
    </form>

    <template v-if="group">
      <section
        v-if="preferences"
        class="rounded-md border border-slate-200 p-5 dark:border-slate-800"
        aria-labelledby="progress-privacy-heading"
      >
        <h2 id="progress-privacy-heading" class="text-sm font-semibold">Your progress privacy</h2>
        <template v-if="preferences.share_progress">
          <p class="mt-2 text-sm leading-6 text-slate-600 dark:text-slate-400">
            Your progress in this group is shared with the owner and all current and future leaders.
            Other members cannot see it. Stopping sharing removes live leader access, not copies
            already downloaded.
          </p>
          <p
            v-if="preferences.notice_version !== GROUP_PROGRESS_NOTICE_VERSION"
            class="mt-2 text-sm"
          >
            This sharing was accepted with a different notice version. You can stop sharing below.
          </p>
          <button
            type="button"
            class="secondary-button mt-3"
            :disabled="!canWrite"
            @click="share(false)"
          >
            Stop sharing progress
          </button>
        </template>
        <template v-else>
          <p class="mt-2 text-sm text-slate-600 dark:text-slate-400">
            Your completion is private. Sharing is optional and does not affect participation.
          </p>
          <label class="mt-3 flex items-start gap-3 text-sm leading-6"
            ><input
              v-model="acknowledgement"
              type="checkbox"
              class="mt-1.5 shrink-0"
              :disabled="!canWrite"
            /><span
              >I agree to share my progress in this group with the owner and all current and future
              leaders. Peers cannot see it. I can stop sharing at any time; downloaded copies cannot
              be recalled.</span
            ></label
          >
          <button
            type="button"
            class="secondary-button mt-3"
            :disabled="!canWrite || !acknowledgement"
            @click="share(true)"
          >
            Enable progress sharing
          </button>
        </template>
      </section>
      <nav class="flex flex-wrap gap-2" aria-label="Assignment lifecycle">
        <button
          type="button"
          :class="lifecycle === 'open' ? 'primary-button' : 'secondary-button'"
          :aria-pressed="lifecycle === 'open'"
          :disabled="loading || busy"
          @click="assignments.changeState('open')"
        >
          Open assignments</button
        ><button
          type="button"
          :class="lifecycle === 'closed' ? 'primary-button' : 'secondary-button'"
          :aria-pressed="lifecycle === 'closed'"
          :disabled="loading || busy"
          @click="assignments.changeState('closed')"
        >
          Closed history
        </button>
      </nav>
      <p v-if="lifecycle === 'closed'" class="text-sm text-slate-600 dark:text-slate-400">
        Closed history is not active work. Your completion is preserved.
      </p>
      <p v-if="!items.length && !loading" class="text-sm text-slate-600 dark:text-slate-400">
        {{ lifecycle === 'open' ? 'No open assignments.' : 'No closed assignments.' }}
      </p>
      <div class="space-y-4">
        <article
          v-for="row in items"
          :key="row.id"
          class="min-w-0 rounded-md border border-slate-200 p-5 dark:border-slate-800"
          :aria-labelledby="`assignment-${row.id}`"
        >
          <div class="flex flex-wrap items-start justify-between gap-3">
            <h2
              :id="`assignment-${row.id}`"
              class="min-w-0 break-words font-semibold [overflow-wrap:anywhere]"
            >
              {{ row.title }}
            </h2>
            <span class="text-xs font-medium text-slate-600 dark:text-slate-400">{{
              row.state === 'closed' ? 'Closed history' : 'Open'
            }}</span>
          </div>
          <p class="mt-2 break-words text-sm text-slate-600 dark:text-slate-400">
            {{ deadline(row) }}
          </p>
          <details v-if="row.description" class="mt-3 text-sm">
            <summary class="cursor-pointer py-2 font-medium">Instructions</summary>
            <div class="mt-2">
              <SafeMarkdown
                :source="row.description"
                formatting-only
                class="min-w-0 break-words [&_pre]:max-w-full [&_pre]:overflow-x-auto [&_table]:block [&_table]:max-w-full [&_table]:overflow-x-auto"
              />
            </div>
          </details>
          <p class="mt-3 text-sm font-medium">
            {{
              row.my_progress.completed_at
                ? 'You completed this assignment.'
                : 'Your work is not completed.'
            }}
          </p>
          <p
            v-if="row.changed_since_completion"
            class="mt-2 text-sm text-amber-800 dark:text-amber-300"
          >
            The assignment changed after your completion. Your completion is preserved; reopen your
            work if needed.
          </p>
          <div class="mt-4 flex flex-wrap gap-2">
            <button
              class="secondary-button"
              type="button"
              :disabled="!canWrite"
              @click="complete(row)"
            >
              {{ row.my_progress.completed_at ? 'Reopen my work' : 'Mark complete' }}</button
            ><template v-if="leader"
              ><button
                class="secondary-button"
                type="button"
                :disabled="!canWrite || !!draft"
                @click="beginDraft(row)"
              >
                Edit</button
              ><button
                class="secondary-button"
                type="button"
                :disabled="!canWrite"
                @click="assignments.showReport(row)"
              >
                View shared progress</button
              ><button
                class="secondary-button text-rose-700 dark:text-rose-300"
                type="button"
                :disabled="!canWrite || draft?.id === row.id"
                @click="remove(row)"
              >
                Delete
              </button></template
            >
          </div>
        </article>
      </div>
      <button
        v-if="next"
        type="button"
        class="secondary-button"
        :disabled="!canWrite"
        @click="assignments.more()"
      >
        Load more assignments
      </button>
      <section
        v-if="report && leader"
        class="rounded-md border border-slate-300 p-5 dark:border-slate-700"
        aria-labelledby="shared-progress-heading"
      >
        <div class="flex flex-wrap justify-between gap-3">
          <h2 id="shared-progress-heading" class="font-semibold">Shared progress</h2>
          <button type="button" class="secondary-button" @click="report = null">
            Close report
          </button>
        </div>
        <p class="mt-3 break-words text-sm font-medium">{{ report.assignment.title }}</p>
        <p class="mt-2 text-sm leading-6 text-slate-600 dark:text-slate-400">
          Only members who opted in are listed. This is not a class-wide completion report. Refresh
          rechecks current access.
        </p>
        <p v-if="!report.data.items.length" class="mt-4 text-sm">
          No members are sharing progress for this assignment.
        </p>
        <ul class="mt-4 divide-y divide-slate-200 dark:divide-slate-800">
          <li
            v-for="row in report.data.items"
            :key="row.user_id"
            class="flex flex-wrap justify-between gap-2 py-3 text-sm"
          >
            <span class="break-words">@{{ row.handle }}</span
            ><span
              >{{ row.completed_at ? 'Completed' : 'Not completed'
              }}{{
                row.completed_at &&
                row.seen_assignment_version != null &&
                row.seen_assignment_version < report.assignment.version
                  ? ' · Definition changed'
                  : ''
              }}</span
            >
          </li>
        </ul>
      </section>
      <section
        v-if="leader && undos.length"
        class="rounded-md border border-slate-200 p-5 dark:border-slate-800"
        aria-labelledby="assignment-recovery-heading"
      >
        <h2 id="assignment-recovery-heading" class="text-sm font-semibold">
          Recent assignment deletions
        </h2>
        <p class="mt-2 text-sm text-slate-600 dark:text-slate-400">
          Undo lasts 60 seconds and requires unchanged group membership. There is no
          permanent-delete fallback.
        </p>
        <ul class="mt-3 space-y-3">
          <li
            v-for="receipt in undos"
            :key="receipt.id"
            class="flex flex-wrap items-center justify-between gap-3 text-sm"
          >
            <span class="min-w-0 break-words">{{ receipt.title }}</span
            ><button
              class="secondary-button"
              type="button"
              :aria-label="`Undo deletion of ${receipt.title}`"
              :disabled="!canWrite || Date.parse(receipt.undo_until) <= now"
              @click="restore(receipt)"
            >
              {{ Date.parse(receipt.undo_until) > now ? 'Undo deletion' : 'Undo expired' }}
            </button>
          </li>
        </ul>
        <button
          v-if="nextUndo"
          class="secondary-button mt-3"
          type="button"
          :disabled="!canWrite"
          @click="assignments.more(true)"
        >
          Load more deletions
        </button>
      </section>
      <details v-if="usage" class="text-xs leading-5 text-slate-600 dark:text-slate-400">
        <summary class="cursor-pointer py-2">Group content capacity</summary>
        <p class="mt-2">
          {{ usage.used_records.toLocaleString() }} /
          {{ usage.max_records.toLocaleString() }} logical records ·
          {{ (usage.used_bytes / 1048576).toFixed(2) }} /
          {{ (usage.max_bytes / 1048576).toFixed(0) }} MiB. Includes reserved progress capacity, not
          a count of completed work. Existing completion remains available when full. Recoverable
          deletions retain capacity until purge.
        </p>
      </details>
    </template>
  </div>
</template>
