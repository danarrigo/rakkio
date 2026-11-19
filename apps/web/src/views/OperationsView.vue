<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { RotateCw } from '@lucide/vue'
import { ApiError } from '../api/client'
import { getOperationsSnapshot } from '../api/client'
import type { OperationsSnapshot } from '../api/types'
import { formatBytes } from '../file-usage'
import OperationsUptime from '../components/OperationsUptime.vue'
import { useWorkspaceStore } from '../stores/workspace'

const store = useWorkspaceStore()
const snapshot = ref<OperationsSnapshot | null>(null)
const loading = ref(false)
const error = ref('')
const denied = ref(false)
const stale = ref(false)
const lastRefresh = ref<string | null>(null)
const metrics = computed(() => snapshot.value?.metrics)
let mounted = false
let poll: ReturnType<typeof setTimeout> | undefined
let request: AbortController | null = null

function timestamp(value: string | null | undefined, empty = 'No data') {
  return value ? new Date(value).toLocaleString() : empty
}

function stop() {
  clearTimeout(poll)
  request?.abort()
  request = null
  loading.value = false
}

async function refresh() {
  if (!mounted || document.hidden || request || denied.value) return
  clearTimeout(poll)
  const controller = new AbortController()
  request = controller
  loading.value = true
  const timeout = setTimeout(() => controller.abort(), 15_000)
  try {
    const result = await getOperationsSnapshot(controller.signal)
    if (!mounted || request !== controller || document.hidden) return
    snapshot.value = result
    lastRefresh.value = new Date().toISOString()
    stale.value = false
    error.value = ''
  } catch (cause) {
    if (!mounted || request !== controller || document.hidden) return
    stale.value = true
    if (cause instanceof ApiError && (cause.status === 401 || cause.status === 403)) {
      denied.value = true
      snapshot.value = null
      lastRefresh.value = null
      store.operationsAllowed = false
      error.value =
        cause.status === 401
          ? 'Sign in again to access Operations.'
          : 'Operations is restricted to the owner. Your account does not have access.'
    } else {
      error.value =
        'Could not refresh Operations. Service status is unknown; any previous snapshot is stale.'
    }
  } finally {
    clearTimeout(timeout)
    if (request === controller) {
      request = null
      loading.value = false
      if (mounted && !document.hidden && !denied.value)
        poll = setTimeout(() => void refresh(), 60_000)
    }
  }
}

function visibilityChanged() {
  if (document.hidden) {
    stop()
    stale.value = true
  } else {
    void refresh()
  }
}

onMounted(() => {
  mounted = true
  document.addEventListener('visibilitychange', visibilityChanged)
  void refresh()
})
onBeforeUnmount(() => {
  mounted = false
  stop()
  document.removeEventListener('visibilitychange', visibilityChanged)
})
</script>

<template>
  <section
    class="mx-auto max-w-6xl px-5 py-10 sm:px-8 lg:px-12 lg:py-14"
    aria-labelledby="operations-title"
  >
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div>
        <h1 id="operations-title" class="page-title !mt-0">Operations</h1>
        <p class="page-description">
          Service-wide snapshot. Read-only, not a live monitoring guarantee.
        </p>
      </div>
      <button
        v-if="!denied"
        class="secondary-button"
        type="button"
        :disabled="loading"
        @click="refresh"
      >
        <RotateCw :size="16" :class="{ 'animate-spin': loading }" />
        {{ loading ? 'Refreshing…' : error ? 'Retry refresh' : 'Refresh snapshot' }}
      </button>
    </div>

    <p class="mt-4 text-xs text-slate-500 dark:text-slate-400" role="status">
      Last successful refresh: {{ timestamp(lastRefresh, 'Not yet received') }}.
      <template v-if="!denied">Refreshes every 60 seconds while visible.</template>
      <strong v-if="stale && snapshot" class="text-amber-700 dark:text-amber-300">
        Stale snapshot.</strong
      >
    </p>
    <p
      v-if="error"
      role="alert"
      class="mt-4 rounded-lg border border-amber-300 bg-amber-50 p-4 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
    >
      {{ error }}
    </p>

    <OperationsUptime v-if="!denied && store.operationsAllowed" />

    <template v-if="snapshot && !denied">
      <p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
        As of {{ timestamp(snapshot.as_of) }}. All values below describe that snapshot.
      </p>
      <p
        v-if="snapshot.database !== 'ok' || !metrics"
        role="alert"
        class="mt-4 rounded-lg border border-amber-300 p-4 text-sm text-amber-800 dark:border-amber-800 dark:text-amber-200"
      >
        Degraded snapshot.
        {{
          snapshot.database !== 'ok'
            ? 'Database probe unavailable.'
            : 'Database probe responded, but aggregate query failed.'
        }}
        Usage and sync metrics are unknown, not zero.
      </p>

      <div class="mt-6 grid gap-4 md:grid-cols-2">
        <article class="operations-card">
          <h2>API &amp; database</h2>
          <dl>
            <div>
              <dt>API response</dt>
              <dd>{{ snapshot.api === 'ok' ? 'Responding' : 'Unavailable' }}</dd>
            </div>
            <div>
              <dt>Database probe</dt>
              <dd>{{ snapshot.database === 'ok' ? 'Responding' : 'Unavailable' }}</dd>
            </div>
          </dl>
          <p>
            Database status is a SELECT 1 probe, not a backup or replication check. If session
            authentication cannot reach the database, this page cannot fetch a snapshot.
          </p>
          <a href="https://dashboard.render.com/" target="_blank" rel="noopener noreferrer"
            >Open hosting console ↗</a
          >
        </article>

        <article class="operations-card">
          <h2>Accounts</h2>
          <dl>
            <div>
              <dt>Current accounts</dt>
              <dd>{{ metrics?.accounts ?? 'Unknown' }}</dd>
            </div>
            <div>
              <dt>Configured account cap</dt>
              <dd>{{ snapshot.limits.max_user_accounts ?? 'Unlimited - not configured' }}</dd>
            </div>
          </dl>
          <p
            v-if="
              metrics &&
              snapshot.limits.max_user_accounts != null &&
              metrics.accounts >= snapshot.limits.max_user_accounts
            "
            class="!text-amber-700 dark:!text-amber-300"
          >
            Account cap reached. New accounts are blocked; existing users can still sign in.
          </p>
          <p>
            Counts registered accounts, not active sessions. Registration settings are managed
            separately.
          </p>
        </article>

        <article class="operations-card">
          <h2>File capacity</h2>
          <dl>
            <div>
              <dt>Service-wide recorded usage</dt>
              <dd>{{ metrics ? formatBytes(metrics.file_bytes) : 'Unknown' }}</dd>
            </div>
            <div>
              <dt>Global quota</dt>
              <dd>{{ formatBytes(snapshot.limits.max_total_file_storage_bytes) }}</dd>
            </div>
            <div>
              <dt>Per-account quota</dt>
              <dd>{{ formatBytes(snapshot.limits.max_user_file_storage_bytes) }}</dd>
            </div>
            <div>
              <dt>Per-file limit</dt>
              <dd>{{ formatBytes(snapshot.limits.max_file_size_bytes) }}</dd>
            </div>
          </dl>
          <p
            v-if="metrics && metrics.file_bytes >= snapshot.limits.max_total_file_storage_bytes"
            class="!text-amber-700 dark:!text-amber-300"
          >
            Global quota reached. Review storage in the provider console before changing capacity.
          </p>
          <p>
            Database attachment metadata only. Not actual R2 bucket bytes, provider billing, or
            proof that files can be read.
          </p>
          <a href="https://dash.cloudflare.com/" target="_blank" rel="noopener noreferrer"
            >Open Cloudflare / R2 console ↗</a
          >
        </article>

        <article class="operations-card">
          <h2>Sync queue</h2>
          <dl>
            <div>
              <dt>Pending · current queue</dt>
              <dd>{{ metrics?.pending ?? 'Unknown' }}</dd>
            </div>
            <div>
              <dt>Running · current queue</dt>
              <dd>{{ metrics?.running ?? 'Unknown' }}</dd>
            </div>
            <div>
              <dt>Retryable · current queue</dt>
              <dd>{{ metrics?.retryable ?? 'Unknown' }}</dd>
            </div>
            <div>
              <dt>Oldest waiting · created</dt>
              <dd>
                {{
                  metrics
                    ? timestamp(metrics.oldest_waiting_created_at, 'No waiting jobs')
                    : 'Unknown'
                }}
              </dd>
            </div>
          </dl>
          <p>
            Retryable means failed with fewer than 8 attempts, including scheduled backoff. Waiting
            includes pending and retryable jobs. Running may include an expired lease awaiting
            recovery.
          </p>
        </article>

        <article class="operations-card">
          <h2>Sync outcomes</h2>
          <dl>
            <div>
              <dt>Final failed · all time</dt>
              <dd>{{ metrics?.final_failed_all_time ?? 'Unknown' }}</dd>
            </div>
            <div>
              <dt>Completed · all time</dt>
              <dd>{{ metrics?.succeeded_all_time ?? 'Unknown' }}</dd>
            </div>
            <div>
              <dt>Latest completed job</dt>
              <dd>{{ metrics ? timestamp(metrics.latest_completed_job_at) : 'Unknown' }}</dd>
            </div>
          </dl>
          <p>
            All time means retained job rows; deletion can remove history. Final failures have at
            least 8 attempts and remain counted after later successes.
          </p>
          <p
            v-if="metrics && metrics.final_failed_all_time > 0"
            class="!text-amber-700 dark:!text-amber-300"
          >
            Review worker logs and Google integration configuration. No provider error payloads are
            shown here.
          </p>
          <p>
            A completed job does not prove every calendar is healthy. A successful HTTP worker
            trigger is not provider success.
          </p>
          <a href="https://console.cloud.google.com/" target="_blank" rel="noopener noreferrer"
            >Open Google Cloud console ↗</a
          >
        </article>

        <article class="operations-card">
          <h2>Not verified</h2>
          <dl>
            <div>
              <dt>R2 read / write health</dt>
              <dd>Unknown</dd>
            </div>
            <div>
              <dt>Backups &amp; restore evidence</dt>
              <dd>Unknown</dd>
            </div>
            <div>
              <dt>External alert delivery</dt>
              <dd>Unknown</dd>
            </div>
            <div>
              <dt>Budget &amp; provider billing</dt>
              <dd>Unknown</dd>
            </div>
          </dl>
          <p>
            No provider probes, backup verification or external alerts run from this page. Verify
            backups and restore tests in the database console; check billing and alerts directly
            with each provider.
          </p>
          <a href="https://console.neon.tech/" target="_blank" rel="noopener noreferrer"
            >Open Neon database console ↗</a
          >
          <br />
          <a
            href="https://github.com/danarrigo/rakkio/actions"
            target="_blank"
            rel="noopener noreferrer"
            >Open workflow runs ↗</a
          >
        </article>
      </div>
    </template>
  </section>
</template>

<style scoped>
@reference '../style.css';
.operations-card {
  @apply min-w-0 rounded-lg border border-slate-200 p-5 dark:border-slate-800;
}
.operations-card h2 {
  @apply text-base font-semibold;
}
.operations-card dl {
  @apply mt-4 space-y-3 text-sm;
}
.operations-card dl > div {
  @apply flex flex-wrap justify-between gap-x-4 gap-y-1;
}
.operations-card dt {
  @apply text-slate-600 dark:text-slate-400;
}
.operations-card dd {
  @apply font-medium;
}
.operations-card p {
  @apply mt-4 text-xs leading-relaxed text-slate-500 dark:text-slate-400;
}
.operations-card a {
  @apply mt-4 inline-block text-sm font-medium underline underline-offset-4;
}
</style>
