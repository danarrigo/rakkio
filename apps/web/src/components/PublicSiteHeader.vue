<script setup lang="ts">
import { RouterLink } from 'vue-router'
import BrandLogo from './BrandLogo.vue'
</script>

<template>
  <header class="border-b border-slate-200 dark:border-slate-800">
    <div class="mx-auto flex h-16 max-w-6xl items-center justify-between px-5 sm:px-8">
      <RouterLink
        class="shrink-0 rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2"
        to="/"
      >
        <BrandLogo />
      </RouterLink>
      <nav aria-label="Public navigation" class="flex items-center gap-5 text-xs text-slate-500">
        <RouterLink class="hover:text-slate-950 dark:hover:text-white" to="/privacy">
          Privacy
        </RouterLink>
        <RouterLink class="hover:text-slate-950 dark:hover:text-white" to="/terms">
          Terms
        </RouterLink>
        <a
          class="hover:text-slate-950 dark:hover:text-white"
          href="https://github.com/danarrigo/rakkio"
          rel="noreferrer"
          target="_blank"
        >
          Source
        </a>
      </nav>
    </div>
  </header>
</template>
