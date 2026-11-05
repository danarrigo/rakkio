import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import SafeMarkdown from './SafeMarkdown.vue'

describe('shared formatting-only Markdown', () => {
  it('keeps readable formatting and explicit links, without loading embedded media', () => {
    const wrapper = mount(SafeMarkdown, {
      props: {
        formattingOnly: true,
        source:
          '**Important**\n\n[Reading](https://example.test/reading)\n\n```text\nexample\n```\n<img src="https://example.test/pixel"><video src="https://example.test/video"></video><audio src="https://example.test/audio"></audio><svg><image href="https://example.test/svg"></image></svg>',
      },
    })
    expect(wrapper.find('strong').text()).toBe('Important')
    expect(wrapper.find('a').attributes('href')).toBe('https://example.test/reading')
    expect(wrapper.find('pre code').text()).toContain('example')
    expect(wrapper.findAll('img,video,audio,svg,source,iframe')).toHaveLength(0)
  })
  it('removes interactive HTML, script URLs and handlers from shared instructions', () => {
    const wrapper = mount(SafeMarkdown, {
      props: {
        formattingOnly: true,
        source:
          '<form action="/api/v1/tasks"><input name="title"><button>Submit</button></form><script>alert(1)</script><a href="javascript:alert(1)" onclick="alert(2)">Unsafe link</a>',
      },
    })
    expect(wrapper.findAll('form,input,button,script')).toHaveLength(0)
    expect(wrapper.find('a').attributes('href')).toBeUndefined()
    expect(wrapper.find('a').attributes('onclick')).toBeUndefined()
  })
  it('does not change existing personal Markdown media behavior and reacts to policy changes', async () => {
    const wrapper = mount(SafeMarkdown, {
      props: { source: '![Personal image](https://example.test/image)' },
    })
    expect(wrapper.find('img').exists()).toBe(true)
    await wrapper.setProps({ formattingOnly: true })
    expect(wrapper.find('img').exists()).toBe(false)
  })
})
