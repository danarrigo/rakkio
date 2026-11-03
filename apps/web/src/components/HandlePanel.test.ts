import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as api from '../api/client'
import type { HandleProfile } from '../api/types'
import { useWorkspaceStore } from '../stores/workspace'
import HandlePanel from './HandlePanel.vue'

vi.mock('../api/client', () => ({ getHandle: vi.fn(), claimHandle: vi.fn() }))
const wrappers: ReturnType<typeof mount>[] = []
function user(id = 'owner') {
  return {
    id,
    display_name: 'Owner',
    email: 'owner@example.test',
    avatar_url: null,
    timezone: 'UTC',
  }
}
function panel() {
  const wrapper = mount(HandlePanel, { global: { stubs: { RouterLink: true } } })
  wrappers.push(wrapper)
  return wrapper
}
async function choose(wrapper: ReturnType<typeof mount>, value = '@DaNiEl') {
  await wrapper.get('input[name="handle"]').setValue(value)
  await wrapper.get('input[type="checkbox"]').setValue(true)
}
beforeEach(() => {
  vi.resetAllMocks()
  setActivePinia(createPinia())
  useWorkspaceStore().user = user()
  vi.mocked(api.getHandle).mockResolvedValue({ handle: null })
})
afterEach(() => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount()
})

describe('HandlePanel', () => {
  it('claims the canonical handle only after an explicit permanent/public choice', async () => {
    vi.mocked(api.claimHandle).mockResolvedValue({ handle: 'daniel' })
    const wrapper = panel()
    await flushPromises()
    expect(wrapper.get('button[type="submit"]').attributes('disabled')).toBeDefined()
    await wrapper.get('input[name="handle"]').setValue('@DaNiEl')
    expect(wrapper.text()).toContain('@daniel')
    expect(wrapper.get('button[type="submit"]').attributes('disabled')).toBeDefined()
    await wrapper.get('input[type="checkbox"]').setValue(true)
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(api.claimHandle).toHaveBeenCalledWith(
      { handle: 'daniel', expected_user_id: 'owner' },
      expect.any(AbortSignal),
    )
    expect(wrapper.text()).toContain('Your handle has been claimed')
    expect(wrapper.find('form').exists()).toBe(false)
  })
  it('clears acknowledgement when the chosen handle changes and rejects confusable input', async () => {
    const wrapper = panel()
    await flushPromises()
    await choose(wrapper)
    await wrapper.get('input[name="handle"]').setValue('different')
    expect(wrapper.get('input[type="checkbox"]').element).toHaveProperty('checked', false)
    await choose(wrapper, 'Ａlice')
    await wrapper.get('form').trigger('submit')
    expect(wrapper.get('button[type="submit"]').attributes('disabled')).toBeDefined()
    expect(api.claimHandle).not.toHaveBeenCalled()
  })
  it('keeps existing handles read-only', async () => {
    vi.mocked(api.getHandle).mockResolvedValue({ handle: 'daniel' })
    const wrapper = panel()
    await flushPromises()
    expect(wrapper.text()).toContain('@daniel')
    expect(wrapper.find('form').exists()).toBe(false)
    expect(api.claimHandle).not.toHaveBeenCalled()
  })
  it('requires a known status before showing the claim form', async () => {
    vi.mocked(api.getHandle).mockRejectedValueOnce(new Error('Unavailable.'))
    const wrapper = panel()
    await flushPromises()
    expect(wrapper.find('form').exists()).toBe(false)
    expect(wrapper.get('[role="alert"]').text()).toContain('Refresh')
    await wrapper.get('button').trigger('click')
    await flushPromises()
    expect(wrapper.find('form').exists()).toBe(true)
  })
  it('requires refreshing uncertain writes and never retries a claim automatically', async () => {
    vi.mocked(api.claimHandle).mockRejectedValue(new Error('Request not confirmed.'))
    const wrapper = panel()
    await flushPromises()
    await choose(wrapper)
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(wrapper.get('button[type="submit"]').attributes('disabled')).toBeDefined()
    expect(api.claimHandle).toHaveBeenCalledOnce()
    vi.mocked(api.getHandle).mockResolvedValue({ handle: 'daniel' })
    await wrapper.get('button').trigger('click')
    await flushPromises()
    expect(wrapper.find('form').exists()).toBe(false)
    expect(api.claimHandle).toHaveBeenCalledOnce()
  })
  it('discards a late claim after switching accounts', async () => {
    let finish: ((value: HandleProfile) => void) | undefined
    vi.mocked(api.claimHandle).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const wrapper = panel()
    await flushPromises()
    await choose(wrapper)
    await wrapper.get('form').trigger('submit')
    const oldSignal = vi.mocked(api.claimHandle).mock.calls[0]?.[1]
    vi.mocked(api.getHandle).mockResolvedValue({ handle: 'other' })
    useWorkspaceStore().user = user('other')
    await flushPromises()
    finish?.({ handle: 'daniel' })
    await flushPromises()
    expect(oldSignal?.aborted).toBe(true)
    expect(wrapper.text()).toContain('@other')
    expect(wrapper.text()).not.toContain('@daniel')
    expect(wrapper.text()).not.toContain('Your handle has been claimed')
  })
  it('discards late reads after account switches and aborts on unmount', async () => {
    let finish: ((value: HandleProfile) => void) | undefined
    vi.mocked(api.getHandle).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const wrapper = panel()
    await flushPromises()
    const signal = vi.mocked(api.getHandle).mock.calls[0]?.[0]
    useWorkspaceStore().user = user('other')
    await flushPromises()
    finish?.({ handle: 'old_owner' })
    await flushPromises()
    expect(signal?.aborted).toBe(true)
    expect(wrapper.text()).not.toContain('@old_owner')
    const latest = vi.mocked(api.getHandle).mock.calls.at(-1)?.[0]
    wrapper.unmount()
    expect(latest?.aborted).toBe(true)
  })
})
