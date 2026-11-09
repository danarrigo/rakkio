<script setup lang="ts">
import { RouterLink } from 'vue-router'
import PublicPageShell from '../components/PublicPageShell.vue'
</script>

<template>
  <PublicPageShell
    title="Delete your account"
    eyebrow="Account help"
    summary="You can delete your rakkio account in Settings or contact the operator if you cannot sign in."
  >
    <section>
      <h2>Delete from Settings</h2>
      <ol>
        <li><RouterLink to="/">Sign in</RouterLink> with the Google account you use for rakkio.</li>
        <li>Open Settings and find the Delete account section.</li>
        <li>Choose Delete account and type <strong>DELETE</strong> when prompted.</li>
      </ol>
      <p>
        If you own a group, transfer its ownership or delete it first and wait for its 60-second
        Undo window to expire. If an upload or external cleanup is still pending, follow the error
        message and retry deletion after cleanup finishes.
      </p>
    </section>
    <section>
      <h2>What deletion removes</h2>
      <p>
        Account deletion removes your account and user-owned records from active rakkio systems,
        removes attachment objects and revokes stored Google authorization. Shared group definitions
        do not disappear simply because a former leader deletes their account.
      </p>
      <p>
        See the <RouterLink to="/privacy">Privacy Policy</RouterLink> for the retention rules
        covering backups, security records and handle reservations.
      </p>
    </section>
    <section>
      <h2>If you cannot sign in</h2>
      <p>
        Email <a href="mailto:daniel.manurung.dev@gmail.com">daniel.manurung.dev@gmail.com</a>
        from your account email to request deletion. The operator may need to verify your identity.
        Never send passwords, session cookies or access tokens.
      </p>
    </section>
  </PublicPageShell>
</template>
