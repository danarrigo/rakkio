<script setup lang="ts">
import { RouterLink } from 'vue-router'
import PublicPageShell from '../components/PublicPageShell.vue'
</script>

<template>
  <PublicPageShell
    title="Page not found"
    eyebrow="404"
    summary="This address does not point to a current rakkio page."
  >
    <p>The page may have moved or no longer exist.</p>
    <p><RouterLink to="/">Return to rakkio</RouterLink></p>
  </PublicPageShell>
</template>
