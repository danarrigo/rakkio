<script setup lang="ts">
import { RouterLink } from 'vue-router'
import BrandLogo from './BrandLogo.vue'

withDefaults(
  defineProps<{
    title: string
    summary: string
    lastUpdated?: string
    eyebrow?: string
  }>(),
  { eyebrow: 'Legal', lastUpdated: '' },
)
</script>

<template>
  <div class="min-h-dvh bg-white text-slate-950 dark:bg-slate-950 dark:text-slate-50">
    <header class="border-b border-slate-200 dark:border-slate-800">
      <div class="mx-auto flex h-16 max-w-5xl items-center justify-between px-5 sm:px-8">
        <RouterLink
          class="shrink-0 rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2"
          to="/"
        >
          <BrandLogo />
        </RouterLink>
        <nav aria-label="Legal pages" class="flex items-center gap-5 text-xs text-slate-500">
          <RouterLink class="hover:text-slate-950 dark:hover:text-white" to="/privacy">
            Privacy
          </RouterLink>
          <RouterLink class="hover:text-slate-950 dark:hover:text-white" to="/terms">
            Terms
          </RouterLink>
        </nav>
      </div>
    </header>

    <main id="public-document" class="mx-auto max-w-3xl px-5 py-14 sm:px-8 sm:py-20">
      <p class="text-xs font-semibold uppercase tracking-[0.18em] text-slate-400">{{ eyebrow }}</p>
      <h1 class="mt-4 text-3xl font-semibold tracking-[-0.04em] sm:text-4xl">{{ title }}</h1>
      <p class="mt-4 max-w-2xl text-sm leading-7 text-slate-500 dark:text-slate-400">
        {{ summary }}
      </p>
      <p v-if="lastUpdated" class="mt-3 text-xs text-slate-400">Last updated {{ lastUpdated }}</p>

      <article class="public-document mt-12 border-t border-slate-200 pt-10 dark:border-slate-800">
        <slot />
      </article>
    </main>

    <footer class="border-t border-slate-200 dark:border-slate-800">
      <div
        class="mx-auto flex max-w-5xl flex-col gap-3 px-5 py-8 text-xs text-slate-500 sm:flex-row sm:items-center sm:justify-between sm:px-8"
      >
        <span>rakkio is independently operated by Daniel in Indonesia.</span>
        <a
          class="hover:text-slate-950 dark:hover:text-white"
          href="mailto:daniel.manurung.dev@gmail.com"
        >
          daniel.manurung.dev@gmail.com
        </a>
      </div>
    </footer>
  </div>
</template>

<style scoped>
.public-document :deep(section + section) {
  margin-top: 2.5rem;
}

.public-document :deep(h2) {
  font-size: 1rem;
  font-weight: 600;
  letter-spacing: -0.01em;
}

.public-document :deep(h3) {
  margin-top: 1.5rem;
  font-size: 0.875rem;
  font-weight: 600;
}

.public-document :deep(p),
.public-document :deep(li) {
  color: rgb(100 116 139);
  font-size: 0.875rem;
  line-height: 1.75;
}

:global(.dark) .public-document :deep(p),
:global(.dark) .public-document :deep(li) {
  color: rgb(148 163 184);
}

.public-document :deep(h2 + p),
.public-document :deep(h3 + p),
.public-document :deep(p + p),
.public-document :deep(p + ul),
.public-document :deep(p + ol) {
  margin-top: 0.75rem;
}

.public-document :deep(ul),
.public-document :deep(ol) {
  margin-top: 0.75rem;
  padding-left: 1.25rem;
}

.public-document :deep(ul) {
  list-style: disc;
}

.public-document :deep(ol) {
  list-style: decimal;
}

.public-document :deep(li + li) {
  margin-top: 0.4rem;
}

.public-document :deep(a) {
  color: inherit;
  text-decoration: underline;
  text-underline-offset: 0.2rem;
}

.public-document :deep(strong) {
  color: rgb(15 23 42);
  font-weight: 600;
}

:global(.dark) .public-document :deep(strong) {
  color: rgb(241 245 249);
}
</style>
