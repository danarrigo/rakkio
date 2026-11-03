<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { Download, Trash2, Upload } from '@lucide/vue'
import * as api from '../api/client'
import DiscordPanel from '../components/DiscordPanel.vue'
import HandlePanel from '../components/HandlePanel.vue'
import GoogleSyncPanel from '../components/GoogleSyncPanel.vue'
import GoogleTasksPanel from '../components/GoogleTasksPanel.vue'
import type { SyncConflictPolicy, ThemePreference } from '../api/types'
import { useWorkspaceStore } from '../stores/workspace'
import { parseTodoistCsv, todoistProjectName, type ParsedTodoistImport } from '../todoist-import'

const store = useWorkspaceStore()
const theme = ref<ThemePreference>('system')
const automaticReview = ref(true)
const sidebarVisible = ref(true)
const conflictPolicy = ref<SyncConflictPolicy>('ask')
const saving = ref(false)
const saveError = ref('')
const saveButton = ref<HTMLButtonElement | null>(null)
const todoistImport = ref<ParsedTodoistImport | null>(null)
const todoistProject = ref('')
const todoistImportBusy = ref(false)
const todoistImportMessage = ref('')
const todoistImportError = ref('')

watch(
  () => store.settings,
  (settings) => {
    if (!settings) return
    theme.value = settings.theme
    automaticReview.value = settings.automatic_daily_review
    sidebarVisible.value = settings.sidebar_visible
    conflictPolicy.value = settings.sync_conflict_policy
  },
  { immediate: true },
)

async function save() {
  if (!store.settings || saving.value) return
  saving.value = true
  saveError.value = ''
  try {
    await store.saveSettings({
      ...store.settings,
      theme: theme.value,
      automatic_daily_review: automaticReview.value,
      sync_conflict_policy: conflictPolicy.value,
      sidebar_visible: sidebarVisible.value,
    })
    if (theme.value === 'system') localStorage.removeItem('prosepect.theme')
    else localStorage.setItem('prosepect.theme', theme.value)
    const dark =
      theme.value === 'dark' ||
      (theme.value === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)
    document.documentElement.classList.toggle('dark', dark)
  } catch (cause) {
    saveError.value =
      cause instanceof Error ? cause.message : 'Could not save settings. Please retry.'
  } finally {
    saving.value = false
    await nextTick()
    saveButton.value?.focus()
  }
}

async function selectTodoistCsv(event: Event) {
  todoistImport.value = null
  todoistImportMessage.value = ''
  todoistImportError.value = ''
  const input = event.currentTarget as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  if (file.size > 10 * 1024 * 1024) {
    todoistImportError.value = 'Todoist CSV must not exceed 10 MiB.'
    return
  }
  try {
    todoistProject.value = todoistProjectName(file.name)
    todoistImport.value = parseTodoistCsv(await file.text(), todoistProject.value)
  } catch (cause) {
    todoistImportError.value =
      cause instanceof Error ? cause.message : 'Could not read Todoist CSV.'
  }
}

async function importTodoistProject() {
  if (!todoistImport.value) return
  todoistImportBusy.value = true
  todoistImportMessage.value = ''
  todoistImportError.value = ''
  try {
    const result = await api.importTodoist({
      ...todoistImport.value.request,
      project_name: todoistProject.value.trim() || 'Todoist import',
    })
    await store.refresh()
    store.selectedProjectId = result.project.id
    todoistImportMessage.value = `${result.imported_tasks} tasks imported into ${result.project.name}.`
    todoistImport.value = null
  } catch (cause) {
    todoistImportError.value = cause instanceof Error ? cause.message : 'Todoist import failed.'
  } finally {
    todoistImportBusy.value = false
  }
}

async function deleteAccount() {
  const confirmation = window.prompt('Type DELETE to permanently remove your account and data.')
  if (confirmation !== 'DELETE') return
  await store.deleteAccount()
}
</script>

<template>
  <div class="mx-auto max-w-4xl px-5 py-10 sm:px-8 lg:px-12 lg:py-14">
    <h1 class="page-title !mt-0">Settings</h1>
    <p class="page-description">Preferences, portable exports, and account controls.</p>

    <form
      class="mt-10 border-y border-slate-200 py-6 dark:border-slate-800"
      aria-label="Workspace settings"
      @submit.prevent="save"
    >
      <h2 class="text-sm font-semibold">Preferences</h2>
      <div class="mt-5 grid gap-5 sm:grid-cols-2">
        <label>
          <span class="field-label">Theme</span>
          <select v-model="theme" :disabled="saving" class="field-input">
            <option value="system">Use system setting</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </label>
        <label>
          <span class="field-label">Synchronization conflicts</span>
          <select v-model="conflictPolicy" :disabled="saving" class="field-input">
            <option value="ask">Always ask</option>
            <option value="latest">Keep most recently edited</option>
            <option value="google">Prefer Google</option>
            <option value="prosepect">Prefer rakkio</option>
          </select>
        </label>
      </div>
      <label class="mt-5 flex items-start gap-3 text-sm">
        <input v-model="automaticReview" :disabled="saving" class="mt-0.5" type="checkbox" />
        <span>
          <strong class="font-medium">Start the daily review automatically</strong>
          <span class="mt-1 block text-xs leading-5 text-slate-500 dark:text-slate-400">
            Prompt for carry-forward decisions when yesterday has unfinished focus tasks.
          </span>
        </span>
      </label>
      <label class="mt-5 flex items-start gap-3 text-sm">
        <input v-model="sidebarVisible" :disabled="saving" class="mt-0.5" type="checkbox" />
        <span>
          <strong class="font-medium">Show sidebar</strong>
          <span class="mt-1 block text-xs leading-5 text-slate-500 dark:text-slate-400">
            Turn this off for a full-width, command-focused workspace. Return here with
            <kbd>G</kbd> then <kbd>S</kbd>, or use <kbd>Ctrl</kbd>/<kbd>⌘</kbd> + <kbd>K</kbd>.
          </span>
        </span>
      </label>
      <p v-if="saveError" role="alert" class="mt-5 text-sm text-rose-600">{{ saveError }}</p>
      <div class="mt-6 flex justify-end">
        <button
          ref="saveButton"
          class="primary-button"
          type="submit"
          :disabled="saving || !store.settings"
        >
          {{ saving ? 'Saving…' : 'Save settings' }}
        </button>
      </div>
    </form>

    <HandlePanel />
    <DiscordPanel />
    <GoogleSyncPanel />
    <GoogleTasksPanel />

    <section class="border-b border-slate-200 py-8 dark:border-slate-800">
      <h2 class="text-sm font-semibold">Import from Todoist</h2>
      <p class="mt-2 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
        Import an official Todoist project CSV. Tasks, subtasks, descriptions, labels, priorities,
        dates, simple recurrence, comments, sections, and durations are mapped before anything is
        saved.
      </p>
      <div class="mt-5 flex flex-wrap items-center gap-3">
        <label class="secondary-button cursor-pointer">
          <Upload :size="15" /> Choose Todoist CSV
          <input class="sr-only" type="file" accept=".csv,text/csv" @change="selectTodoistCsv" />
        </label>
        <span v-if="todoistImport" class="text-xs text-slate-500">
          {{ todoistImport.report.importedTasks }} tasks ready
        </span>
      </div>

      <div v-if="todoistImport" class="mt-5 border border-slate-200 p-4 dark:border-slate-800">
        <label class="field-label">
          New project name
          <input v-model="todoistProject" class="field-input mt-1" maxlength="120" />
        </label>
        <dl class="mt-4 grid grid-cols-2 gap-3 text-xs sm:grid-cols-4">
          <div>
            <dt class="text-slate-400">Tasks</dt>
            <dd class="mt-1 font-medium">{{ todoistImport.report.importedTasks }}</dd>
          </div>
          <div>
            <dt class="text-slate-400">Comments</dt>
            <dd class="mt-1 font-medium">{{ todoistImport.report.importedComments }}</dd>
          </div>
          <div>
            <dt class="text-slate-400">Sections</dt>
            <dd class="mt-1 font-medium">{{ todoistImport.report.importedSections }}</dd>
          </div>
          <div>
            <dt class="text-slate-400">Skipped rows</dt>
            <dd class="mt-1 font-medium">{{ todoistImport.report.skippedRows }}</dd>
          </div>
        </dl>
        <ul
          v-if="todoistImport.report.warnings.length"
          class="mt-4 max-h-32 list-disc overflow-y-auto pl-5 text-xs leading-5 text-amber-700 dark:text-amber-400"
        >
          <li v-for="warning in todoistImport.report.warnings" :key="warning">{{ warning }}</li>
        </ul>
        <div class="mt-5 flex justify-end">
          <button
            class="primary-button"
            type="button"
            :disabled="todoistImportBusy || !todoistImport.report.importedTasks"
            @click="importTodoistProject"
          >
            {{ todoistImportBusy ? 'Importing…' : 'Import project' }}
          </button>
        </div>
      </div>
      <p v-if="todoistImportError" class="mt-4 text-xs text-rose-600" role="alert">
        {{ todoistImportError }}
      </p>
      <p v-if="todoistImportMessage" class="mt-4 text-xs text-emerald-700" aria-live="polite">
        {{ todoistImportMessage }}
      </p>
    </section>

    <section class="border-b border-slate-200 py-8 dark:border-slate-800">
      <h2 class="text-sm font-semibold">Export your data</h2>
      <p class="mt-2 text-sm text-slate-500 dark:text-slate-400">
        Download portable copies without losing access to your workspace.
      </p>
      <div class="mt-5 grid gap-2 sm:grid-cols-2">
        <a class="secondary-button justify-start" :href="api.apiUrl('/api/v1/exports/json')"
          ><Download :size="15" /> Complete JSON</a
        >
        <a class="secondary-button justify-start" :href="api.apiUrl('/api/v1/exports/tasks.csv')"
          ><Download :size="15" /> Tasks CSV</a
        >
        <a class="secondary-button justify-start" :href="api.apiUrl('/api/v1/exports/notes.md')"
          ><Download :size="15" /> Notes Markdown</a
        >
        <a
          class="secondary-button justify-start"
          :href="api.apiUrl('/api/v1/exports/calendars.ics')"
          ><Download :size="15" /> Calendars ICS</a
        >
      </div>
    </section>

    <section class="border-b border-slate-200 py-8 dark:border-slate-800">
      <h2 class="text-sm font-semibold">Legal and privacy</h2>
      <p class="mt-2 text-sm text-slate-500 dark:text-slate-400">
        Review how rakkio handles your data and the terms for using the hosted service.
      </p>
      <div class="mt-4 flex gap-4 text-sm">
        <RouterLink class="underline underline-offset-4" to="/privacy">Privacy Policy</RouterLink>
        <RouterLink class="underline underline-offset-4" to="/terms">Terms of Service</RouterLink>
      </div>
    </section>

    <section class="py-8">
      <h2 class="text-sm font-semibold text-rose-700 dark:text-rose-400">Delete account</h2>
      <p class="mt-2 max-w-2xl text-sm leading-6 text-slate-500 dark:text-slate-400">
        Permanently deletes projects, tasks, notes, calendars, integration credentials, sessions,
        and associated files. This cannot be undone.
      </p>
      <button
        class="secondary-button mt-5 text-rose-700 hover:!border-rose-300 hover:!text-rose-800 dark:text-rose-400"
        type="button"
        @click="deleteAccount"
      >
        <Trash2 :size="15" /> Delete my account
      </button>
    </section>
  </div>
</template>
