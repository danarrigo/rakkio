<script setup lang="ts">
import { RouterLink } from 'vue-router'
</script>

<template>
  <footer class="border-t border-slate-200 dark:border-slate-800">
    <div
      class="mx-auto flex max-w-6xl flex-col gap-3 px-5 py-8 text-xs text-slate-500 sm:flex-row sm:items-center sm:justify-between sm:px-8"
    >
      <span>rakkio is independently operated by Daniel in Indonesia.</span>
      <RouterLink class="hover:text-slate-950 dark:hover:text-white" to="/delete-account">
        Delete account
      </RouterLink>
      <a
        class="hover:text-slate-950 dark:hover:text-white"
        href="mailto:daniel.manurung.dev@gmail.com"
      >
        daniel.manurung.dev@gmail.com
      </a>
    </div>
  </footer>
</template>
