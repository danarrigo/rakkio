<script setup lang="ts">
import { watch } from 'vue'
import { useWorkspaceStore } from '../stores/workspace'
import { formatBytes } from '../file-usage'

const store = useWorkspaceStore()
watch(
  () => store.user?.id,
  (actor) => {
    if (actor) void store.refreshFileUsage()
  },
  { immediate: true },
)
</script>

<template>
  <div class="mt-3 text-xs leading-5 text-slate-500 dark:text-slate-400">
    <p v-if="!store.user || store.fileUsageLoading" role="status">Checking attachment storage…</p>
    <p v-else-if="store.fileUsageError" role="alert">
      {{ store.fileUsageError }}
      <button type="button" class="underline" @click="store.refreshFileUsage()">
        Retry limits
      </button>
    </p>
    <template v-else-if="store.fileUsage">
      <p>
        {{ formatBytes(store.fileUsage.used_bytes) }} of
        {{ formatBytes(store.fileUsage.max_user_storage_bytes) }} used
      </p>
      <p>
        Up to {{ formatBytes(store.fileUsage.max_file_size_bytes) }} per file. Shared service
        capacity is checked on upload.
      </p>
    </template>
  </div>
</template>
