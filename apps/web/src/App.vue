<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import {
  Activity,
  Bell,
  CircleHelp,
  LogOut,
  Menu,
  Moon,
  RotateCw,
  Settings as SettingsIcon,
  Sun,
  Users,
  X,
} from '@lucide/vue'
import * as api from './api/client'
import AppSidebar from './components/AppSidebar.vue'
import BrandLogo from './components/BrandLogo.vue'
import CalendarMoveFeedback from './components/CalendarMoveFeedback.vue'
import TaskDeletionFeedback from './components/TaskDeletionFeedback.vue'
import CreateTaskDialog from './components/CreateTaskDialog.vue'
import DailyReviewDialog from './components/DailyReviewDialog.vue'
import GlobalSearch from './components/GlobalSearch.vue'
import KeyboardPalette from './components/KeyboardPalette.vue'
import PublicHome from './components/PublicHome.vue'
import {
  isEditableTarget,
  resolveKeyboardShortcut,
  type GlobalKeyboardAction,
  type KeyboardCommandId,
} from './keyboard'
import { dueInAppReminders, parseDismissedReminders, reminderKey } from './reminders'
import { useWorkspaceStore } from './stores/workspace'
import { useGroupsStore } from './stores/groups'

const store = useWorkspaceStore()
const groups = useGroupsStore()
const route = useRoute()
const router = useRouter()
const sidebarOpen = ref(false)
const navigationToggle = ref<HTMLButtonElement | null>(null)
const dark = ref(false)
const media = window.matchMedia('(prefers-color-scheme: dark)')
const now = ref(Date.now())
const dismissedReminders = ref<string[]>(loadDismissedReminders())
let reminderTimer: ReturnType<typeof setInterval> | undefined
const reportedReminders = new Set<string>()
const globalSearch = ref<{ focus: () => void } | null>(null)
const createTaskDialog = ref<{ open: () => void } | null>(null)
const mainContent = ref<HTMLElement | null>(null)
const paletteMode = ref<'commands' | 'help' | null>(null)
const awaitingGo = ref(false)
let goSequenceTimer: ReturnType<typeof setTimeout> | undefined

const dueReminders = computed(() =>
  dueInAppReminders(store.tasks, now.value, dismissedReminders.value),
)
const sidebarVisible = computed(() => store.settings?.sidebar_visible ?? false)
const publicRoute = computed(() => route.meta.public === true)
const undoFeedbackVisible = computed(() =>
  Boolean(
    store.calendarMoveUndo ||
    store.calendarMovePending ||
    store.calendarMoveMessage ||
    store.calendarMoveError ||
    store.taskDeleteUndos.length ||
    store.taskDeletePending ||
    store.taskDeleteMessage ||
    store.taskDeleteError,
  ),
)

watch(dueReminders, (tasks) => {
  for (const task of tasks) {
    const key = reminderKey(task.id, task.remind_at!)
    if (reportedReminders.has(key)) continue
    reportedReminders.add(key)
    void api.recordReminderDelivery().catch(() => reportedReminders.delete(key))
  }
})

const pageTitle = computed(() => {
  if (route.name === 'inbox') return 'Inbox'
  if (route.name === 'calendar') return 'Calendar'
  if (route.name === 'projects') return 'Projects'
  if (route.name === 'notes') return 'Notes'
  if (route.name === 'files') return 'Files'
  if (route.name === 'operations') return 'Operations'
  if (route.name === 'settings') return 'Settings'
  if (route.name === 'groups' || route.name === 'group-assignments') return 'Groups'
  return 'Today'
})

async function closeSidebar() {
  const wasOpen = sidebarOpen.value
  sidebarOpen.value = false
  if (wasOpen) {
    await nextTick()
    navigationToggle.value?.focus()
  }
}

function applyTheme() {
  const preference = localStorage.getItem('prosepect.theme')
  dark.value = preference ? preference === 'dark' : media.matches
  document.documentElement.classList.toggle('dark', dark.value)
}

function toggleTheme() {
  localStorage.setItem('prosepect.theme', dark.value ? 'light' : 'dark')
  applyTheme()
}

function handleSystemTheme() {
  if (!localStorage.getItem('prosepect.theme')) applyTheme()
}

function dismissReminder(taskId: string, remindAt: string) {
  dismissedReminders.value = [...dismissedReminders.value, reminderKey(taskId, remindAt)].slice(
    -500,
  )
  localStorage.setItem('prosepect.dismissed-reminders', JSON.stringify(dismissedReminders.value))
}

function loadDismissedReminders() {
  return parseDismissedReminders(localStorage.getItem('prosepect.dismissed-reminders'))
}

function setAwaitingGo(value: boolean) {
  awaitingGo.value = value
  if (goSequenceTimer) clearTimeout(goSequenceTimer)
  if (value) goSequenceTimer = setTimeout(() => (awaitingGo.value = false), 1_200)
}

async function focusMainContent() {
  await nextTick()
  mainContent.value?.focus()
}

async function executeCommand(command: KeyboardCommandId) {
  paletteMode.value = null
  if (command === 'navigate-today') {
    await router.push('/')
    await focusMainContent()
  } else if (command === 'navigate-inbox') {
    await router.push('/inbox')
    await focusMainContent()
  } else if (command === 'navigate-projects') {
    store.selectProject(null)
    await router.push('/projects')
    await focusMainContent()
  } else if (command === 'navigate-calendar') {
    await router.push('/calendar')
    await focusMainContent()
  } else if (command === 'navigate-notes') {
    await router.push('/notes')
    await focusMainContent()
  } else if (command === 'navigate-settings') {
    await router.push('/settings')
    await focusMainContent()
  } else if (command === 'create-task') {
    await nextTick()
    createTaskDialog.value?.open()
  } else if (command === 'create-event') {
    const query =
      route.name === 'calendar' ? { ...route.query, action: 'new-event' } : { action: 'new-event' }
    await router.push({ name: 'calendar', query })
  } else if (command === 'focus-search') {
    await nextTick()
    globalSearch.value?.focus()
  }
}

function runKeyboardAction(action: GlobalKeyboardAction) {
  if (action === 'open-command-palette') paletteMode.value = 'commands'
  else if (action === 'open-shortcut-help') paletteMode.value = 'help'
  else void executeCommand(action)
}

function handleGlobalKeydown(event: KeyboardEvent) {
  if (
    publicRoute.value ||
    store.authenticationRequired ||
    paletteMode.value ||
    document.querySelector('[role="dialog"]')
  ) {
    return
  }
  const resolution = resolveKeyboardShortcut(
    event,
    awaitingGo.value,
    isEditableTarget(event.target),
  )
  setAwaitingGo(resolution.awaitingGo)
  if (!resolution.handled) return
  event.preventDefault()
  if (resolution.action) runKeyboardAction(resolution.action)
}

onMounted(() => {
  applyTheme()
  media.addEventListener('change', handleSystemTheme)
  reminderTimer = setInterval(() => (now.value = Date.now()), 30_000)
  window.addEventListener('keydown', handleGlobalKeydown)
  void store.bootstrap()
})

onBeforeUnmount(() => {
  media.removeEventListener('change', handleSystemTheme)
  window.removeEventListener('keydown', handleGlobalKeydown)
  if (reminderTimer) clearInterval(reminderTimer)
  if (goSequenceTimer) clearTimeout(goSequenceTimer)
})
</script>

<template>
  <RouterView v-if="publicRoute" />

  <PublicHome v-else-if="store.authenticationRequired" />

  <div v-else class="flex min-h-dvh bg-white text-slate-950 dark:bg-slate-950 dark:text-slate-50">
    <a
      class="fixed left-4 top-3 z-[90] -translate-y-20 bg-slate-950 px-3 py-2 text-sm font-medium text-white transition focus:translate-y-0 dark:bg-white dark:text-slate-950"
      href="#workspace-content"
    >
      Skip to content
    </a>
    <DailyReviewDialog />
    <KeyboardPalette
      :open="paletteMode !== null"
      :mode="paletteMode ?? 'commands'"
      @close="paletteMode = null"
      @select="executeCommand"
    />
    <div
      v-if="sidebarVisible && sidebarOpen"
      class="fixed inset-0 z-30 bg-slate-950/35 backdrop-blur-[1px] lg:hidden"
      aria-hidden="true"
      @click="closeSidebar"
    />
    <AppSidebar v-if="sidebarVisible" :open="sidebarOpen" @close="closeSidebar" />

    <div class="min-w-0 flex-1">
      <header
        class="sticky top-0 z-20 flex h-14 items-center border-b border-slate-200 bg-white px-3 dark:border-slate-800 dark:bg-slate-950 sm:px-5 lg:px-8"
      >
        <button
          v-if="sidebarVisible"
          ref="navigationToggle"
          class="icon-button mr-3 lg:hidden"
          type="button"
          aria-label="Open navigation"
          aria-controls="primary-navigation"
          :aria-expanded="sidebarOpen"
          @click="sidebarOpen = true"
        >
          <Menu :size="19" />
        </button>
        <RouterLink
          class="w-[120px] shrink-0 rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2"
          :class="{ 'lg:hidden': sidebarVisible }"
          to="/"
        >
          <BrandLogo />
        </RouterLink>
        <span
          class="text-xs font-medium text-slate-500 dark:text-slate-400"
          :class="sidebarVisible ? 'hidden lg:inline' : 'ml-4 hidden xl:inline'"
          >{{ pageTitle }}</span
        >
        <div class="ml-auto flex items-center gap-1 sm:gap-2">
          <GlobalSearch ref="globalSearch" />
          <CreateTaskDialog
            ref="createTaskDialog"
            :default-project-id="route.name === 'inbox' ? null : undefined"
          />
          <button
            v-if="store.settings && !sidebarVisible"
            class="icon-button"
            type="button"
            aria-label="Open settings"
            title="Settings (G then S)"
            @click="executeCommand('navigate-settings')"
          >
            <SettingsIcon :size="17" />
          </button>
          <button
            class="icon-button"
            type="button"
            aria-label="Show keyboard shortcuts"
            title="Keyboard shortcuts (?)"
            @click="paletteMode = 'help'"
          >
            <CircleHelp :size="17" />
          </button>
          <RouterLink
            v-if="groups.enabled && !sidebarVisible"
            class="icon-button"
            to="/groups"
            aria-label="Groups"
            title="Groups"
            ><Users :size="17"
          /></RouterLink>
          <RouterLink
            v-if="store.operationsAllowed && !sidebarVisible"
            class="icon-button"
            to="/operations"
            aria-label="Operations"
            title="Operations"
          >
            <Activity :size="17" />
          </RouterLink>
          <button
            v-if="store.user"
            class="icon-button"
            type="button"
            aria-label="Sign out"
            title="Sign out"
            @click="store.logout"
          >
            <LogOut :size="17" />
          </button>
          <button
            class="icon-button"
            type="button"
            :aria-label="dark ? 'Use light theme' : 'Use dark theme'"
            @click="toggleTheme"
          >
            <Sun v-if="dark" :size="18" />
            <Moon v-else :size="18" />
          </button>
        </div>
      </header>

      <p
        v-if="groups.enabled && groups.pendingCredential"
        role="status"
        class="mx-5 mt-4 text-sm sm:mx-8"
      >
        You have a group invitation to review.
        <RouterLink to="/groups" class="underline underline-offset-4">Review invitation</RouterLink>
      </p>
      <div
        v-if="undoFeedbackVisible"
        class="mx-auto grid max-h-[45dvh] w-full max-w-xl gap-2 overflow-y-auto overscroll-contain p-3"
        aria-label="Undo feedback"
      >
        <CalendarMoveFeedback />
        <TaskDeletionFeedback />
      </div>

      <div
        v-if="store.error"
        class="mx-5 mt-4 flex items-start gap-3 border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-800 dark:border-rose-900 dark:bg-rose-950/50 dark:text-rose-200 sm:mx-8"
        role="alert"
      >
        <span class="min-w-0 flex-1">{{ store.error }}</span>
        <button type="button" aria-label="Dismiss error" @click="store.clearError">
          <X :size="17" />
        </button>
      </div>

      <div
        v-for="task in route.name === 'operations' ? [] : dueReminders"
        :key="reminderKey(task.id, task.remind_at!)"
        class="mx-5 mt-4 flex items-center gap-3 border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-100 sm:mx-8"
        role="status"
      >
        <Bell :size="17" class="shrink-0" />
        <span class="min-w-0 flex-1"><strong>Reminder:</strong> {{ task.title }}</span>
        <button
          type="button"
          :aria-label="`Dismiss reminder for ${task.title}`"
          @click="dismissReminder(task.id, task.remind_at!)"
        >
          <X :size="17" />
        </button>
      </div>

      <main id="workspace-content" ref="mainContent" tabindex="-1" class="outline-none">
        <div v-if="store.loading" class="grid min-h-[calc(100dvh-3.5rem)] place-items-center">
          <div class="flex items-center gap-3 text-sm text-slate-500">
            <RotateCw :size="18" class="animate-spin" />
            Loading…
          </div>
        </div>
        <RouterView v-else :key="store.user?.id" />
      </main>
    </div>

    <div
      v-if="awaitingGo"
      class="fixed bottom-5 left-1/2 z-[70] -translate-x-1/2 border border-slate-300 bg-white px-3 py-2 text-xs font-medium shadow-lg dark:border-slate-700 dark:bg-slate-900"
      role="status"
    >
      <kbd>G</kbd> then <kbd>T</kbd>, <kbd>P</kbd>, <kbd>C</kbd>, <kbd>N</kbd>, or <kbd>S</kbd>
    </div>
  </div>
</template>
