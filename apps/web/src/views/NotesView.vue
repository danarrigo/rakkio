<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { FilePlus2, Pencil, Trash2 } from '@lucide/vue'
import { useRoute } from 'vue-router'
import type { Note } from '../api/types'
import AttachmentPanel from '../components/AttachmentPanel.vue'
import SafeMarkdown from '../components/SafeMarkdown.vue'
import { useWorkspaceStore } from '../stores/workspace'

const store = useWorkspaceStore()
const route = useRoute()
const selectedId = ref<string | null>(null)
const editing = ref(false)
const draftBase = ref<Note | null>(null)
const saving = ref(false)
const error = ref('')
const titleInput = ref<HTMLInputElement | null>(null)
const editButton = ref<HTMLButtonElement | null>(null)
const saveButton = ref<HTMLButtonElement | null>(null)
type LinkKind = 'standalone' | 'project' | 'task' | 'event'

const title = ref('')
const markdown = ref('')
const linkKind = ref<LinkKind>('standalone')
const linkedId = ref('')
const selected = computed(() => store.notes.find((note) => note.id === selectedId.value) ?? null)
const linkOptions = computed(() => {
  if (linkKind.value === 'project') {
    return store.projects.map((project) => ({ id: project.id, label: project.name }))
  }
  if (linkKind.value === 'task') {
    return store.tasks.map((task) => ({ id: task.id, label: task.title }))
  }
  if (linkKind.value === 'event') {
    return store.events.map((event) => ({ id: event.id, label: event.title }))
  }
  return []
})

watch(
  [() => store.notes, () => route.query.note] as const,
  ([notes, requestedId]) => {
    const requested =
      typeof requestedId === 'string' ? notes.find((note) => note.id === requestedId) : null
    if (requested && requested.id !== selectedId.value && !editing.value) selectNote(requested)
    else if (!selectedId.value && notes.length && !editing.value) selectNote(notes[0]!)
  },
  { immediate: true },
)

watch(editing, async (value) => {
  if (value) {
    await nextTick()
    titleInput.value?.focus()
  }
})

function selectNote(note: Note) {
  error.value = ''
  selectedId.value = note.id
  title.value = note.title
  markdown.value = note.markdown
  if (note.project_id) {
    linkKind.value = 'project'
    linkedId.value = note.project_id
  } else if (note.task_id) {
    linkKind.value = 'task'
    linkedId.value = note.task_id
  } else if (note.event_id) {
    linkKind.value = 'event'
    linkedId.value = note.event_id
  } else {
    linkKind.value = 'standalone'
    linkedId.value = ''
  }
  editing.value = false
}

function beginEdit() {
  if (!selected.value) return
  selectNote(selected.value)
  draftBase.value = { ...selected.value }
  editing.value = true
}

async function cancel() {
  if (selected.value) selectNote(selected.value)
  await nextTick()
  editButton.value?.focus()
}

function newNote() {
  error.value = ''
  draftBase.value = null
  selectedId.value = null
  title.value = ''
  markdown.value = ''
  linkKind.value = store.selectedProjectId ? 'project' : 'standalone'
  linkedId.value = store.selectedProjectId ?? ''
  editing.value = true
}

async function save() {
  if (saving.value || !title.value.trim()) return
  saving.value = true
  error.value = ''
  const links = {
    project_id: linkKind.value === 'project' ? linkedId.value : undefined,
    task_id: linkKind.value === 'task' ? linkedId.value : undefined,
    event_id: linkKind.value === 'event' ? linkedId.value : undefined,
  }
  try {
    if (draftBase.value) {
      const updated = await store.editNote(
        draftBase.value,
        title.value.trim(),
        markdown.value,
        links,
      )
      selectNote(updated)
    } else {
      const note = await store.addNote({
        project_id: links.project_id ?? null,
        task_id: links.task_id ?? null,
        event_id: links.event_id ?? null,
        title: title.value.trim(),
        markdown: markdown.value,
      })
      selectNote(note)
    }
    await nextTick()
    editButton.value?.focus()
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : 'Could not save this note. Please retry.'
  } finally {
    saving.value = false
    if (error.value) {
      await nextTick()
      saveButton.value?.focus()
    }
  }
}

async function remove() {
  if (!selected.value || !window.confirm(`Delete “${selected.value.title}”?`)) return
  const note = selected.value
  await store.removeNote(note)
  selectedId.value = null
  const next = store.notes[0]
  if (next) selectNote(next)
  else newNote()
}
</script>

<template>
  <div class="mx-auto max-w-7xl px-5 py-10 sm:px-8 lg:px-12 lg:py-14">
    <div class="flex items-end justify-between gap-4">
      <div>
        <h1 class="page-title !mt-0">Notes</h1>
        <p class="page-description">Markdown notes for ideas, context, and project knowledge.</p>
      </div>
      <button class="primary-button" type="button" :disabled="saving" @click="newNote">
        <FilePlus2 :size="16" /> New note
      </button>
    </div>

    <div
      class="mt-10 grid grid-cols-1 border-y border-slate-200 dark:border-slate-800 lg:min-h-[36rem] lg:grid-cols-[17rem_minmax(0,1fr)]"
    >
      <aside
        class="min-w-0 border-b border-slate-200 py-3 dark:border-slate-800 lg:border-b-0 lg:border-r"
      >
        <button
          v-for="note in store.notes"
          :key="note.id"
          class="block w-full border-l-2 px-4 py-3 text-left transition"
          :class="
            selectedId === note.id
              ? 'border-slate-950 bg-slate-50 dark:border-white dark:bg-slate-900'
              : 'border-transparent hover:bg-slate-50 dark:hover:bg-slate-900'
          "
          type="button"
          :disabled="saving"
          @click="selectNote(note)"
        >
          <span class="block truncate text-sm font-medium">{{ note.title }}</span>
          <span class="mt-1 block text-xs text-slate-400">
            {{
              new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' }).format(
                new Date(note.updated_at),
              )
            }}
          </span>
        </button>
        <p v-if="!store.notes.length" class="px-4 py-10 text-sm text-slate-400">No notes yet.</p>
      </aside>

      <main v-if="store.notes.length || editing" class="min-w-0 p-5 sm:p-8">
        <form v-if="editing" aria-label="Note editor" @submit.prevent="save">
          <div class="flex flex-col gap-4 sm:flex-row">
            <label class="min-w-0 flex-1">
              <span class="field-label">Title</span>
              <input
                ref="titleInput"
                v-model="title"
                :disabled="saving"
                class="field-input text-lg font-medium"
                required
                maxlength="240"
              />
            </label>
            <label class="sm:w-44">
              <span class="field-label">Link to</span>
              <select
                v-model="linkKind"
                :disabled="saving"
                class="field-input"
                @change="linkedId = linkOptions[0]?.id ?? ''"
              >
                <option value="standalone">Nothing</option>
                <option value="project">Project</option>
                <option value="task">Task</option>
                <option value="event">Event</option>
              </select>
            </label>
            <label v-if="linkKind !== 'standalone'" class="sm:w-56">
              <span class="field-label capitalize">{{ linkKind }}</span>
              <select v-model="linkedId" :disabled="saving" class="field-input" required>
                <option v-for="option in linkOptions" :key="option.id" :value="option.id">
                  {{ option.label }}
                </option>
              </select>
            </label>
          </div>
          <div class="mt-5 grid min-w-0 grid-cols-1 gap-5 xl:grid-cols-2">
            <label class="min-w-0">
              <span class="field-label">Markdown</span>
              <textarea
                v-model="markdown"
                :disabled="saving"
                class="field-input min-h-96 resize-y py-3 font-mono text-sm"
                maxlength="100000"
              />
            </label>
            <section class="min-w-0" aria-label="Preview">
              <span class="field-label">Preview</span>
              <SafeMarkdown
                class="markdown-preview mt-2 min-h-96 border border-slate-200 p-5 dark:border-slate-800"
                :source="markdown"
              />
            </section>
          </div>
          <p v-if="error" role="alert" class="mt-5 text-sm text-rose-600">{{ error }}</p>
          <div class="mt-5 flex justify-end gap-2">
            <button
              v-if="selected"
              class="secondary-button"
              type="button"
              :disabled="saving"
              @click="cancel"
            >
              Cancel
            </button>
            <button
              ref="saveButton"
              class="primary-button"
              type="submit"
              :disabled="saving || !title.trim()"
            >
              {{ saving ? 'Saving…' : 'Save note' }}
            </button>
          </div>
        </form>

        <article v-else-if="selected">
          <div
            class="flex items-start justify-between gap-4 border-b border-slate-200 pb-5 dark:border-slate-800"
          >
            <div class="min-w-0 [overflow-wrap:anywhere]">
              <h2 class="text-2xl font-semibold tracking-[-0.03em]">{{ selected.title }}</h2>
              <p class="mt-2 text-xs text-slate-400">
                Updated
                {{
                  new Date(selected.updated_at).toLocaleString(undefined, {
                    dateStyle: 'medium',
                    timeStyle: 'short',
                  })
                }}
              </p>
            </div>
            <div class="flex shrink-0 gap-1">
              <button
                ref="editButton"
                class="icon-button"
                type="button"
                aria-label="Edit note"
                @click="beginEdit"
              >
                <Pencil :size="16" />
              </button>
              <button
                class="icon-button hover:!text-rose-600"
                type="button"
                aria-label="Delete note"
                @click="remove"
              >
                <Trash2 :size="16" />
              </button>
            </div>
          </div>
          <SafeMarkdown class="markdown-preview py-7" :source="selected.markdown" />
          <div class="mt-8">
            <AttachmentPanel kind="note" :parent-id="selected.id" />
          </div>
        </article>

        <div v-else class="grid min-h-96 place-items-center text-center">
          <div><p class="text-sm text-slate-400">Select a note or create your first one.</p></div>
        </div>
      </main>
    </div>
  </div>
</template>

<style scoped>
.markdown-preview {
  overflow-wrap: anywhere;
}
.markdown-preview :deep(pre),
.markdown-preview :deep(table) {
  max-width: 100%;
  overflow-x: auto;
  overflow-wrap: normal;
}
.markdown-preview :deep(table) {
  display: block;
}
.markdown-preview :deep(img) {
  max-width: 100%;
}

.markdown-preview :deep(h1),
.markdown-preview :deep(h2),
.markdown-preview :deep(h3) {
  margin: 1.5em 0 0.5em;
  font-weight: 650;
  line-height: 1.25;
}
.markdown-preview :deep(p),
.markdown-preview :deep(ul),
.markdown-preview :deep(ol) {
  margin: 0.75em 0;
  line-height: 1.75;
}
.markdown-preview :deep(ul),
.markdown-preview :deep(ol) {
  padding-left: 1.5rem;
}
.markdown-preview :deep(code) {
  border-radius: 0.25rem;
  background: rgb(148 163 184 / 0.15);
  padding: 0.125rem 0.3rem;
  font-size: 0.875em;
}
.markdown-preview :deep(a) {
  text-decoration: underline;
}
</style>
