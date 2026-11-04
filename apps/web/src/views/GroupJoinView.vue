<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import PublicPageShell from '../components/PublicPageShell.vue'
import GroupJoinPanel from '../components/GroupJoinPanel.vue'
import { useGroupsStore } from '../stores/groups'
import { useWorkspaceStore } from '../stores/workspace'

const router = useRouter()
const groups = useGroupsStore()
const workspace = useWorkspaceStore()
const error = ref('')
const signedIn = computed(() => !!workspace.user)
const fragment = new URLSearchParams(window.location.hash.slice(1)).get('invite')
if (fragment) {
  if (!groups.rememberInvitation(fragment))
    error.value = 'That invitation link is invalid. Ask a leader for a fresh link.'
  // Fragments are never sent to the server; remove it before further navigation.
  void router.replace({ name: 'group-join', hash: '', query: {} })
}
</script>

<template>
  <PublicPageShell
    title="Group invitation"
    eyebrow="Groups"
    summary="Review the group and joining account before confirming membership."
  >
    <p v-if="error" role="alert">{{ error }}</p>
    <template v-if="!signedIn">
      <p>
        Sign in through the normal rakkio account flow, then open Groups to review this invitation.
        Your invitation is kept in this tab for up to 15 minutes. If browser storage is disabled,
        reopen the original link after signing in.
      </p>
      <p><RouterLink to="/">Sign in to rakkio</RouterLink></p>
    </template>
    <p v-else-if="groups.loading && !groups.known" role="status">Checking group availability…</p>
    <template v-else-if="groups.enabled">
      <GroupJoinPanel />
      <p><RouterLink to="/groups">Open your groups</RouterLink></p>
    </template>
    <template v-else>
      <p>{{ groups.error || 'Groups are not available on this installation yet.' }}</p>
      <button
        type="button"
        class="secondary-button"
        :disabled="groups.loading"
        @click="groups.refresh"
      >
        Refresh availability
      </button>
      <p><RouterLink to="/">Return to rakkio</RouterLink></p>
    </template>
  </PublicPageShell>
</template>
