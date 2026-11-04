<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import {
  Activity,
  CalendarDays,
  FileText,
  FolderKanban,
  LayoutDashboard,
  Inbox,
  Settings,
  Users,
  X,
} from '@lucide/vue'
import { useWorkspaceStore } from '../stores/workspace'
import { useGroupsStore } from '../stores/groups'
import BrandLogo from './BrandLogo.vue'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ close: [] }>()
const closeButton = ref<HTMLButtonElement | null>(null)
const store = useWorkspaceStore()
const groups = useGroupsStore()
const route = useRoute()
const router = useRouter()
const visibleProjects = computed(() =>
  store.projects.filter((project) => project.status !== 'archived'),
)

watch(
  () => props.open,
  async (open) => {
    if (open) {
      await nextTick()
      closeButton.value?.focus()
    }
  },
)

function openProject(projectId: string) {
  store.selectProject(projectId)
  void router.push('/projects')
  emit('close')
}

function showAllProjects() {
  store.selectProject(null)
  emit('close')
}
</script>

<template>
  <aside
    id="primary-navigation"
    class="fixed inset-y-0 left-0 z-40 flex w-60 flex-col border-r border-slate-200 bg-slate-50 px-4 pb-5 pt-4 transition-transform duration-200 dark:border-slate-800 dark:bg-slate-950 lg:static lg:translate-x-0"
    :class="props.open ? 'translate-x-0' : 'invisible -translate-x-full lg:visible'"
    aria-label="Primary navigation"
    @keydown.esc="emit('close')"
  >
    <div class="flex h-10 items-center justify-between px-2">
      <RouterLink
        class="shrink-0 rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2"
        to="/"
        @click="emit('close')"
      >
        <BrandLogo />
      </RouterLink>
      <button
        ref="closeButton"
        class="icon-button lg:hidden"
        type="button"
        aria-label="Close navigation"
        @click="emit('close')"
      >
        <X :size="18" />
      </button>
    </div>

    <nav class="mt-3 space-y-1">
      <RouterLink
        class="nav-item"
        :class="{ active: route.name === 'inbox' }"
        to="/inbox"
        @click="emit('close')"
      >
        <Inbox :size="16" /> Inbox
      </RouterLink>
      <RouterLink
        class="nav-item"
        :class="{ active: route.name === 'today' }"
        to="/"
        @click="emit('close')"
      >
        <LayoutDashboard :size="18" />
        <span>Today</span>
      </RouterLink>
      <RouterLink
        class="nav-item"
        :class="{ active: route.name === 'calendar' }"
        to="/calendar"
        @click="emit('close')"
      >
        <CalendarDays :size="17" />
        <span>Calendar</span>
      </RouterLink>
      <RouterLink
        class="nav-item"
        :class="{ active: route.name === 'notes' }"
        to="/notes"
        @click="emit('close')"
      >
        <FileText :size="17" />
        <span>Notes</span>
        <span v-if="store.notes.length" class="nav-count">{{ store.notes.length }}</span>
      </RouterLink>
      <RouterLink
        class="nav-item"
        :class="{ active: route.name === 'projects' && !store.selectedProjectId }"
        to="/projects"
        @click="showAllProjects"
      >
        <FolderKanban :size="17" />
        <span>Projects</span>
        <span v-if="store.openTasks.length" class="nav-count">{{ store.openTasks.length }}</span>
      </RouterLink>
    </nav>

    <RouterLink
      v-if="groups.enabled"
      class="nav-item mt-2"
      :class="{ active: route.name === 'groups' }"
      to="/groups"
      @click="emit('close')"
      ><Users :size="17" /><span>Groups</span></RouterLink
    >

    <div v-if="visibleProjects.length" class="mt-8 px-2">
      <span class="text-[11px] font-medium text-slate-500 dark:text-slate-400">Your projects</span>
    </div>

    <div class="mt-2 min-h-0 flex-1 overflow-y-auto">
      <button
        v-for="project in visibleProjects"
        :key="project.id"
        class="group flex min-h-10 w-full items-center rounded-md px-2 text-left text-[13px] text-slate-600 transition hover:bg-slate-100 hover:text-slate-950 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-slate-400 dark:text-slate-400 dark:hover:bg-slate-900 dark:hover:text-white"
        :class="{
          'bg-slate-100 !text-slate-950 dark:bg-slate-900 dark:!text-white':
            route.name === 'projects' && store.selectedProjectId === project.id,
        }"
        :aria-current="
          route.name === 'projects' && store.selectedProjectId === project.id ? 'page' : undefined
        "
        type="button"
        @click="openProject(project.id)"
      >
        <span class="truncate">{{ project.name }}</span>
      </button>
    </div>

    <RouterLink
      v-if="store.operationsAllowed"
      class="nav-item mt-3"
      :class="{ active: route.name === 'operations' }"
      to="/operations"
      @click="emit('close')"
    >
      <Activity :size="17" />
      <span>Operations</span>
    </RouterLink>
    <RouterLink
      class="nav-item mt-3"
      :class="{ active: route.name === 'settings' }"
      to="/settings"
      @click="emit('close')"
    >
      <Settings :size="17" />
      <span>Settings</span>
    </RouterLink>
  </aside>
</template>
