<script setup lang="ts">
import PublicSiteFooter from './PublicSiteFooter.vue'
import PublicSiteHeader from './PublicSiteHeader.vue'

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
  <div class="flex min-h-dvh flex-col bg-white text-slate-950 dark:bg-slate-950 dark:text-slate-50">
    <PublicSiteHeader />

    <main id="public-document" class="mx-auto w-full max-w-3xl flex-1 px-5 py-14 sm:px-8 sm:py-20">
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

    <PublicSiteFooter />
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

.dark .public-document :deep(p),
.dark .public-document :deep(li) {
  color: rgb(148 163 184);
}

.public-document :deep(h2 + p),
.public-document :deep(h3 + p),
.public-document :deep(p + p),
.public-document :deep(p + ul),
.public-document :deep(p + ol),
.public-document :deep(ol + p),
.public-document :deep(ul + p) {
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

.dark .public-document :deep(strong) {
  color: rgb(241 245 249);
}
</style>
