<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import * as api from '../api/client'
import type { GroupAction, GroupInviteReference, GroupMember, GroupSummary } from '../api/types'
import GroupJoinPanel from '../components/GroupJoinPanel.vue'
import { useGroupsStore, GROUP_ROSTER_NOTICE_VERSION } from '../stores/groups'
import { useWorkspaceStore } from '../stores/workspace'

const groups = useGroupsStore()
const workspace = useWorkspaceStore()
const selectedHeading = ref<HTMLElement | null>(null)
const name = ref('')
const createAcknowledged = ref(false)
const timezone = ref(workspace.user?.timezone || 'UTC')
const inviteKind = ref<'direct' | 'link' | 'code'>('direct')
const inviteHandle = ref('')
const inviteUses = ref(10)
const issuedCredential = ref('')
const reference = ref<GroupInviteReference | null>(null)
const localError = ref('')
const editing = ref<GroupSummary | null>(null)
const editName = ref('')
const editTimezone = ref('UTC')
const editApproval = ref(true)
const group = computed(() => groups.detail?.group)
const owner = computed(() => group.value?.my_role === 'owner')
const leader = computed(() => ['owner', 'leader'].includes(group.value?.my_role || ''))
const locked = computed(() => !groups.canWrite || !!editing.value)
let alive = true
let generation = 0
function scope() {
  const version = generation,
    actor = workspace.user?.id
  return () => alive && version === generation && actor === workspace.user?.id
}
async function selectGroup(id: string) {
  if (editing.value || groups.loading || groups.busy) return
  const current = scope()
  await groups.select(id)
  if (!current() || groups.detail?.group.id !== id) return
  await nextTick()
  if (!current()) return
  selectedHeading.value?.focus({ preventScroll: true })
  if (window.matchMedia('(max-width: 1023px)').matches)
    selectedHeading.value?.scrollIntoView({ block: 'start' })
}
async function create() {
  if (locked.value || !name.value.trim() || !createAcknowledged.value) return
  const current = scope()
  const result = await groups.write((actor, signal) =>
    api.createGroup(
      {
        expected_user_id: actor,
        name: name.value,
        timezone: timezone.value,
        confirm_roster_visibility: true,
        roster_notice_version: GROUP_ROSTER_NOTICE_VERSION,
      },
      signal,
    ),
  )
  if (result && current()) {
    name.value = ''
    await selectGroup(result.id)
  }
}
async function act(action: GroupAction, selected = group.value) {
  if (!selected || !groups.canWrite) return undefined
  const version = selected.version,
    id = selected.id
  return groups.write((actor, signal) =>
    api.mutateGroup(id, { expected_user_id: actor, expected_version: version, action }, signal),
  )
}
function beginEdit() {
  if (!owner.value || !group.value || locked.value) return
  editing.value = { ...group.value }
  editName.value = group.value.name
  editTimezone.value = group.value.timezone
  editApproval.value = group.value.approval_required
}
async function saveEdit() {
  if (!editing.value || !group.value || !owner.value || !groups.canWrite) return
  if (editing.value.version !== group.value.version) {
    localError.value = 'Review the latest values before applying your draft to a new version.'
    return
  }
  const current = scope()
  const result = await act(
    {
      kind: 'update',
      name: editName.value,
      timezone: editTimezone.value,
      approval_required: editApproval.value,
    },
    editing.value,
  )
  if (result && current()) {
    editing.value = null
    localError.value = ''
  }
}
function adoptCurrentVersion() {
  if (!group.value || !owner.value || !editing.value) return
  if (
    window.confirm(
      'Use the latest group version with your draft values? Review the current name, timezone and joining policy shown below before confirming.',
    )
  )
    editing.value = { ...group.value }
}
async function issue() {
  if (!group.value || locked.value || !leader.value) return
  const current = scope(),
    snapshot = group.value
  issuedCredential.value = ''
  const result = await groups.write((actor, signal) =>
    api.issueGroupInvitation(
      snapshot.id,
      {
        expected_user_id: actor,
        expected_version: snapshot.version,
        kind: inviteKind.value,
        handle: inviteKind.value === 'direct' ? inviteHandle.value : null,
        max_uses: inviteKind.value === 'direct' ? 1 : inviteUses.value,
      },
      signal,
    ),
  )
  if (!result || !current()) return
  if (result.credential) {
    if (result.invitation.kind === 'link') {
      const link = new URL('/join', window.location.origin)
      link.hash = new URLSearchParams({ invite: result.credential }).toString()
      issuedCredential.value = link.href
    } else issuedCredential.value = result.credential
  } else {
    inviteHandle.value = ''
    groups.message = 'Invitation sent. The recipient must accept it.'
  }
}
async function copyCredential() {
  localError.value = ''
  try {
    await navigator.clipboard.writeText(issuedCredential.value)
  } catch {
    localError.value = 'Clipboard unavailable. Select and copy the credential manually.'
  }
}
async function memberAction(member: GroupMember, action: 'role' | 'remove' | 'offer' | 'allow') {
  if (!group.value || locked.value || !owner.value) return
  if (action === 'role') {
    const role = member.role === 'leader' ? 'member' : 'leader'
    if (window.confirm(`Make @${member.handle} a ${role}?`))
      await act({
        kind: 'set_role',
        membership_id: member.id,
        expected_membership_version: member.version,
        role,
      })
  } else if (action === 'remove') {
    if (
      window.confirm(
        `Remove @${member.handle}? Their group access and personal group progress are removed. Existing links cannot add them back until you allow a fresh invitation.`,
      )
    )
      await act({
        kind: 'remove',
        membership_id: member.id,
        expected_membership_version: member.version,
      })
  } else if (action === 'offer') {
    if (
      window.confirm(
        `Offer ownership to @${member.handle}? They must accept. You will remain a leader after transfer.`,
      )
    )
      await act({ kind: 'offer_ownership', membership_id: member.id })
  } else if (
    window.confirm(
      `Allow @${member.handle} to receive a fresh invitation? This does not join them or restore old progress.`,
    )
  )
    await act({
      kind: 'allow_rejoin',
      membership_id: member.id,
      expected_membership_version: member.version,
    })
}
async function revokeInvitation(id: string, version: number) {
  if (!group.value || locked.value || !leader.value) return
  const snapshot = group.value
  await groups.write((actor, signal) =>
    api.revokeGroupInvitation(
      snapshot.id,
      id,
      { expected_user_id: actor, expected_version: version },
      signal,
    ),
  )
}
async function declineInvitation(id: string, version: number) {
  if (locked.value) return
  await groups.write((actor, signal) =>
    api.declineGroupInvitation(id, { expected_user_id: actor, expected_version: version }, signal),
  )
}
async function removeGroup() {
  if (!group.value || locked.value || !owner.value) return
  if (
    window.confirm(
      `Delete ${group.value.name} for everyone? Undo is available for 60 seconds unless membership changes. This does not delete personal tasks or events.`,
    )
  )
    await act({ kind: 'delete', confirmation: 'DELETE GROUP' })
}
async function leave() {
  if (!group.value || locked.value || owner.value) return
  if (
    window.confirm(
      'Leave this group? Your group progress is removed; rejoining starts fresh. Your personal content stays unchanged.',
    )
  )
    await act({ kind: 'leave', expected_membership_version: group.value.my_membership_version })
}
async function reviewJoin(id: string, version: number, approve: boolean) {
  if (locked.value || !leader.value) return
  if (
    window.confirm(
      approve ? 'Approve this member joining the group?' : 'Decline this join request?',
    )
  )
    await act({ kind: 'review_join', request_id: id, expected_request_version: version, approve })
}
watch([name, timezone], () => {
  createAcknowledged.value = false
})
watch(
  () => groups.selectedId,
  () => {
    issuedCredential.value = ''
    reference.value = null
  },
)
watch(
  () => workspace.user?.id,
  () => {
    generation++
    name.value = ''
    timezone.value = workspace.user?.timezone || 'UTC'
    inviteHandle.value = ''
    issuedCredential.value = ''
    reference.value = null
    editing.value = null
    localError.value = ''
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  alive = false
  generation++
})
</script>

<template>
  <div class="mx-auto max-w-6xl px-5 py-10 sm:px-8 lg:px-12 lg:py-14">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div>
        <h1 class="page-title !mt-0">Groups</h1>
        <p class="page-description">Shared planning without sharing your personal workspace.</p>
      </div>
      <button
        class="secondary-button"
        type="button"
        :disabled="groups.loading || groups.busy"
        @click="groups.refresh"
      >
        {{ groups.loading ? 'Refreshing…' : 'Refresh groups' }}
      </button>
    </div>
    <p
      v-if="groups.error || localError"
      role="alert"
      class="mt-5 text-sm text-rose-700 dark:text-rose-400"
    >
      {{ groups.error || localError }}
    </p>
    <p
      v-if="groups.message"
      role="status"
      class="mt-4 text-sm text-emerald-700 dark:text-emerald-400"
    >
      {{ groups.message }}
    </p>
    <p v-if="!groups.known && groups.loading" role="status" class="mt-8 text-sm">
      Checking group availability…
    </p>
    <p
      v-else-if="groups.known && !groups.enabled"
      class="mt-8 text-sm leading-6 text-slate-500 dark:text-slate-400"
    >
      Groups are not available on this installation yet. Your personal workspace is unchanged.
    </p>
    <template v-else-if="groups.enabled">
      <p class="mt-5 text-sm leading-6 text-slate-500 dark:text-slate-400">
        Choose your
        <RouterLink to="/settings" class="underline">@handle in Settings</RouterLink> before
        creating or joining a group. Personal progress stays private unless you explicitly share it.
      </p>
      <div class="mt-8 grid min-w-0 gap-8 lg:grid-cols-[16rem_minmax(0,1fr)]">
        <aside class="min-w-0" aria-label="Your groups">
          <h2 class="text-sm font-semibold">Your groups</h2>
          <p
            v-if="!groups.items.length && !groups.loading"
            class="mt-3 text-sm text-slate-500 dark:text-slate-400"
          >
            No groups yet. Create one or review an invitation.
          </p>
          <ul class="mt-3 space-y-2">
            <li v-for="item in groups.items" :key="item.id" class="min-w-0">
              <template v-if="item.deleted_at">
                <div class="rounded-md border border-slate-200 p-3 dark:border-slate-800">
                  <p class="break-words text-sm">{{ item.name }} - deleted</p>
                  <p class="mt-1 text-xs text-slate-500">Undo requires unchanged membership.</p>
                  <button
                    type="button"
                    class="secondary-button mt-3"
                    :disabled="locked"
                    @click="act({ kind: 'restore' }, item)"
                  >
                    Undo group deletion
                  </button>
                </div>
              </template>
              <button
                v-else
                class="w-full break-words rounded-md border px-3 py-3 text-left text-sm"
                :class="
                  groups.selectedId === item.id
                    ? 'border-slate-500 bg-slate-100 dark:bg-slate-900'
                    : 'border-slate-200 dark:border-slate-800'
                "
                :disabled="groups.loading || groups.busy || !!editing"
                type="button"
                :aria-pressed="groups.selectedId === item.id"
                @click="selectGroup(item.id)"
              >
                {{ item.name
                }}<span class="mt-1 block text-xs text-slate-500 dark:text-slate-400"
                  >{{ item.my_role }} · {{ item.member_count }} members</span
                >
              </button>
            </li>
          </ul>
          <details
            :open="!groups.items.length"
            class="mt-8 border-t border-slate-200 pt-2 dark:border-slate-800"
          >
            <summary class="cursor-pointer py-3 text-sm font-semibold outline-offset-4">
              Create a group
            </summary>
            <form class="mt-3" aria-label="Create group" @submit.prevent="create">
              <h2 class="text-sm font-semibold">Create a group</h2>
              <label class="field-label mt-4" for="group-name">Group name</label
              ><input
                id="group-name"
                v-model="name"
                class="field-input mt-2"
                maxlength="120"
                required
                :disabled="locked"
              />
              <label class="field-label mt-4" for="group-timezone">Group timezone</label
              ><input
                id="group-timezone"
                v-model="timezone"
                class="field-input mt-2"
                maxlength="64"
                required
                :disabled="locked"
              />
              <p class="mt-3 text-xs leading-5 text-slate-500 dark:text-slate-400">
                Members see your display name, @handle and role, not your email or personal content.
                Link/code joining requires leader approval by default. You can own up to 5 groups
                and join up to 20.
              </p>
              <label class="mt-4 flex items-start gap-3 text-sm leading-6"
                ><input
                  v-model="createAcknowledged"
                  type="checkbox"
                  class="mt-1.5 shrink-0"
                  :disabled="locked"
                /><span
                  >I agree that group members can see my display name, @handle and role. Personal
                  content stays private.</span
                ></label
              >
              <button
                class="primary-button mt-4"
                type="submit"
                :disabled="locked || !name.trim() || !createAcknowledged"
              >
                Create group
              </button>
            </form>
          </details>
        </aside>
        <div class="min-w-0 space-y-8">
          <section
            v-if="editing && !group"
            class="rounded-md border border-slate-200 p-5 dark:border-slate-800"
            aria-label="Unsaved group draft"
          >
            <h2 class="text-sm font-semibold">Unsaved group draft</h2>
            <p class="mt-3 text-sm">
              The group is no longer available. Your draft is kept here for copying; it cannot be
              saved.
            </p>
            <label class="field-label mt-4" for="orphaned-group-name">Draft name</label
            ><input
              id="orphaned-group-name"
              :value="editName"
              class="field-input mt-2"
              readonly
            /><label class="field-label mt-4" for="orphaned-group-timezone">Draft timezone</label
            ><input
              id="orphaned-group-timezone"
              :value="editTimezone"
              class="field-input mt-2"
              readonly
            /><button class="secondary-button mt-4" type="button" @click="editing = null">
              Discard draft
            </button>
          </section>
          <section
            v-if="groups.detail && group"
            class="min-w-0 rounded-md border border-slate-200 p-5 dark:border-slate-800"
            aria-labelledby="selected-group-heading"
          >
            <div class="flex flex-wrap items-start justify-between gap-3">
              <div class="min-w-0">
                <h2
                  id="selected-group-heading"
                  ref="selectedHeading"
                  tabindex="-1"
                  class="scroll-mt-20 break-words text-lg font-semibold outline-none"
                >
                  {{ group.name }}
                </h2>
                <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
                  {{ group.timezone }} · {{ group.my_role }} · {{ group.member_count }} members
                </p>
              </div>
              <button
                v-if="owner && !editing"
                class="secondary-button"
                type="button"
                :disabled="locked"
                @click="beginEdit"
              >
                Edit group
              </button>
            </div>
            <RouterLink
              v-if="!editing && !groups.busy"
              :to="{ name: 'group-assignments', params: { groupId: group.id } }"
              class="secondary-button mt-4 inline-flex"
              >View assignments</RouterLink
            >
            <form
              v-if="editing"
              class="mt-5 border-t border-slate-200 pt-4 dark:border-slate-800"
              aria-label="Edit group"
              @submit.prevent="saveEdit"
            >
              <label class="field-label" for="edit-group-name">Group name</label
              ><input
                id="edit-group-name"
                v-model="editName"
                class="field-input mt-2"
                maxlength="120"
                required
                :disabled="groups.busy"
              />
              <label class="field-label mt-4" for="edit-group-timezone">Group timezone</label
              ><input
                id="edit-group-timezone"
                v-model="editTimezone"
                class="field-input mt-2"
                maxlength="64"
                required
                :disabled="groups.busy"
              />
              <label class="mt-4 flex items-start gap-3 text-sm leading-6"
                ><input
                  v-model="editApproval"
                  type="checkbox"
                  class="mt-1.5 shrink-0"
                  :disabled="groups.busy"
                /><span
                  >Require leader approval for link/code joins. When off, anyone with a valid
                  invitation can confirm membership.</span
                ></label
              >
              <p v-if="!owner" class="mt-3 text-sm">
                You no longer own this group. Copy your draft before closing it.
              </p>
              <div v-if="editing.version !== group.version" class="mt-4 text-sm leading-6">
                <p>
                  Latest values: {{ group.name }}, {{ group.timezone }},
                  {{
                    group.approval_required ? 'approval required' : 'immediate confirmed joining'
                  }}. Your draft has not been overwritten.
                </p>
                <button
                  class="secondary-button mt-3"
                  type="button"
                  :disabled="!owner || groups.stale"
                  @click="adoptCurrentVersion"
                >
                  Review and apply draft to latest version
                </button>
              </div>
              <div class="mt-4 flex flex-wrap gap-3">
                <button
                  class="primary-button"
                  type="submit"
                  :disabled="!groups.canWrite || !owner || editing.version !== group.version"
                >
                  Save group</button
                ><button
                  class="secondary-button"
                  type="button"
                  :disabled="groups.busy"
                  @click="editing = null"
                >
                  Cancel edit
                </button>
              </div>
            </form>
            <div
              v-if="groups.detail.ownership_offer"
              class="mt-5 rounded-md border border-amber-300 p-4 text-sm dark:border-amber-800"
            >
              <template
                v-if="groups.detail.ownership_offer.to_membership_id === group.my_membership_id"
                ><p>
                  You have an ownership offer. Accepting gives you responsibility for membership and
                  shared group content.
                </p>
                <button
                  class="primary-button mt-3"
                  type="button"
                  :disabled="locked"
                  @click="
                    act({ kind: 'accept_ownership', offer_id: groups.detail.ownership_offer.id })
                  "
                >
                  Accept ownership
                </button></template
              >
              <template v-else-if="owner"
                ><p>An ownership offer is awaiting acceptance.</p>
                <button
                  class="secondary-button mt-3"
                  type="button"
                  :disabled="locked"
                  @click="act({ kind: 'cancel_ownership' })"
                >
                  Cancel ownership offer
                </button></template
              >
            </div>
            <h3 class="mt-6 text-sm font-semibold">Members</h3>
            <ul class="mt-3 divide-y divide-slate-200 dark:divide-slate-800">
              <li
                v-for="person in groups.detail.members"
                :key="person.id"
                class="flex min-w-0 flex-wrap items-center justify-between gap-3 py-3"
              >
                <div class="min-w-0">
                  <p class="break-words text-sm font-medium">
                    {{ person.display_name || '@' + person.handle }}
                  </p>
                  <p class="break-all text-xs text-slate-500 dark:text-slate-400">
                    @{{ person.handle }} · {{ person.role }}
                  </p>
                </div>
                <div v-if="owner && person.user_id !== group.owner_id" class="flex flex-wrap gap-2">
                  <button
                    class="secondary-button"
                    type="button"
                    :disabled="locked"
                    @click="memberAction(person, 'role')"
                  >
                    {{ person.role === 'leader' ? 'Make member' : 'Make leader' }}</button
                  ><button
                    class="secondary-button"
                    type="button"
                    :disabled="locked"
                    @click="memberAction(person, 'offer')"
                  >
                    Offer ownership</button
                  ><button
                    class="secondary-button"
                    type="button"
                    :disabled="locked"
                    @click="memberAction(person, 'remove')"
                  >
                    Remove
                  </button>
                </div>
              </li>
            </ul>
            <div v-if="owner && groups.detail.removed_members.length" class="mt-5">
              <h3 class="text-sm font-semibold">Removed members</h3>
              <ul>
                <li
                  v-for="person in groups.detail.removed_members"
                  :key="person.id"
                  class="mt-3 flex flex-wrap items-center justify-between gap-3 text-sm"
                >
                  <span class="break-all">@{{ person.handle }}</span
                  ><button
                    class="secondary-button"
                    type="button"
                    :disabled="locked"
                    @click="memberAction(person, 'allow')"
                  >
                    Allow fresh invitation
                  </button>
                </li>
              </ul>
            </div>
            <div v-if="leader" class="mt-6 border-t border-slate-200 pt-5 dark:border-slate-800">
              <h3 class="text-sm font-semibold">Invite members</h3>
              <form class="mt-4" aria-label="Invite group members" @submit.prevent="issue">
                <label class="field-label" for="group-invite-kind">Invitation type</label
                ><select
                  id="group-invite-kind"
                  v-model="inviteKind"
                  class="field-input mt-2"
                  :disabled="locked"
                >
                  <option value="direct">Invite by @handle</option>
                  <option value="link">Invite link</option>
                  <option value="code">Join code</option>
                </select>
                <template v-if="inviteKind === 'direct'"
                  ><label class="field-label mt-4" for="group-invite-handle"
                    >Recipient @handle</label
                  ><input
                    id="group-invite-handle"
                    v-model="inviteHandle"
                    class="field-input mt-2"
                    maxlength="31"
                    required
                    autocomplete="off"
                    :disabled="locked"
                /></template>
                <template v-else
                  ><label class="field-label mt-4" for="group-invite-uses">Maximum joins</label
                  ><input
                    id="group-invite-uses"
                    v-model.number="inviteUses"
                    type="number"
                    min="1"
                    max="100"
                    required
                    class="field-input mt-2"
                    :disabled="locked"
                /></template>
                <p class="mt-3 text-xs leading-5 text-slate-500 dark:text-slate-400">
                  Invitations expire after 7 days and never grant leader privileges. Direct
                  recipients must accept;
                  {{
                    group.approval_required
                      ? 'link/code joins require leader approval.'
                      : 'link/code joins require confirmation.'
                  }}
                </p>
                <button class="primary-button mt-4" type="submit" :disabled="locked">
                  {{ inviteKind === 'direct' ? 'Send invitation' : 'Create invitation' }}
                </button>
              </form>
              <div v-if="issuedCredential" class="mt-4">
                <label class="field-label" for="issued-group-invitation"
                  >Copy now - only shown once</label
                ><textarea
                  id="issued-group-invitation"
                  :value="issuedCredential"
                  class="field-input mt-2 break-all font-mono"
                  readonly
                  rows="3"
                /><button class="secondary-button mt-3" type="button" @click="copyCredential">
                  Copy invitation
                </button>
              </div>
              <ul class="mt-5 space-y-3">
                <li
                  v-for="invitation in groups.detail.invitations"
                  :key="invitation.id"
                  class="flex flex-wrap items-center justify-between gap-3 text-sm"
                >
                  <span
                    >{{ invitation.kind
                    }}{{
                      invitation.recipient_handle ? ' for @' + invitation.recipient_handle : ''
                    }}
                    · {{ invitation.uses }}/{{ invitation.max_uses }} used</span
                  ><button
                    class="secondary-button"
                    type="button"
                    :disabled="locked"
                    @click="revokeInvitation(invitation.id, invitation.version)"
                  >
                    Revoke invitation
                  </button>
                </li>
              </ul>
              <div v-if="groups.detail.join_requests.length" class="mt-6">
                <h3 class="text-sm font-semibold">Join requests</h3>
                <ul>
                  <li
                    v-for="request in groups.detail.join_requests"
                    :key="request.id"
                    class="mt-3 flex flex-wrap items-center justify-between gap-3 text-sm"
                  >
                    <span class="break-all">@{{ request.handle }}</span>
                    <div class="flex gap-2">
                      <button
                        class="secondary-button"
                        type="button"
                        :disabled="locked"
                        @click="reviewJoin(request.id, request.version, true)"
                      >
                        Approve request</button
                      ><button
                        class="secondary-button"
                        type="button"
                        :disabled="locked"
                        @click="reviewJoin(request.id, request.version, false)"
                      >
                        Decline request
                      </button>
                    </div>
                  </li>
                </ul>
              </div>
            </div>
            <div class="mt-6 border-t border-slate-200 pt-5 dark:border-slate-800">
              <button
                v-if="owner"
                class="secondary-button text-rose-700 dark:text-rose-400"
                type="button"
                :disabled="locked"
                @click="removeGroup"
              >
                Delete group</button
              ><button
                v-else
                class="secondary-button"
                type="button"
                :disabled="locked"
                @click="leave"
              >
                Leave group
              </button>
            </div>
          </section>
          <section
            v-if="groups.inbox.invitations.length || groups.inbox.requests.length"
            aria-labelledby="group-inbox-heading"
            class="rounded-md border border-slate-200 p-5 dark:border-slate-800"
          >
            <h2 id="group-inbox-heading" class="text-sm font-semibold">
              Your invitations and requests
            </h2>
            <ul class="mt-4 space-y-4">
              <li
                v-for="invitation in groups.inbox.invitations"
                :key="invitation.id"
                class="flex flex-wrap items-center justify-between gap-3"
              >
                <span class="min-w-0 break-words text-sm">{{ invitation.group_name }}</span>
                <div class="flex flex-wrap gap-2">
                  <button
                    class="secondary-button"
                    type="button"
                    :disabled="locked"
                    @click="reference = { invitation_id: invitation.id }"
                  >
                    Review invitation</button
                  ><button
                    class="secondary-button"
                    type="button"
                    :disabled="locked"
                    @click="declineInvitation(invitation.id, invitation.version)"
                  >
                    Decline invitation
                  </button>
                </div>
              </li>
              <li v-for="request in groups.inbox.requests" :key="request.id" class="text-sm">
                <span class="break-words">{{ request.group_name }}</span
                ><span class="ml-2 text-slate-500 dark:text-slate-400">{{
                  request.state === 'pending'
                    ? 'Awaiting approval'
                    : request.state === 'denied'
                      ? 'Request declined'
                      : 'Invitation closed'
                }}</span>
              </li>
            </ul>
          </section>
          <GroupJoinPanel :reference="reference" />
        </div>
      </div>
    </template>
  </div>
</template>
