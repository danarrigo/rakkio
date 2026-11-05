<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import DOMPurify from 'dompurify'
import { marked } from 'marked'

const props = defineProps<{ source: string; formattingOnly?: boolean }>()
const element = ref<HTMLElement | null>(null)

function render(source: string) {
  if (!element.value) return
  element.value.innerHTML = DOMPurify.sanitize(
    marked.parse(source, { async: false }) as string,
    props.formattingOnly
      ? {
          // Shared instructions are formatting, not an external-media/interactive
          // content channel. In particular, never fetch a leader's tracking image.
          ALLOWED_TAGS: [
            'p',
            'br',
            'hr',
            'h1',
            'h2',
            'h3',
            'h4',
            'h5',
            'h6',
            'ul',
            'ol',
            'li',
            'dl',
            'dt',
            'dd',
            'pre',
            'code',
            'em',
            'strong',
            'b',
            'i',
            's',
            'del',
            'blockquote',
            'a',
            'table',
            'thead',
            'tbody',
            'tfoot',
            'tr',
            'td',
            'th',
            'div',
            'span',
          ],
          ALLOWED_ATTR: ['href', 'title', 'colspan', 'rowspan'],
          ALLOW_DATA_ATTR: false,
        }
      : {},
  )
}

watch([() => props.source, () => props.formattingOnly], () => render(props.source))
onMounted(() => render(props.source))
</script>

<template>
  <div ref="element" />
</template>
