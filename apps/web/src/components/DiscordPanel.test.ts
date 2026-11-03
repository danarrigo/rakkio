import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as api from '../api/client'
import type { DiscordStatus } from '../api/types'
import { useWorkspaceStore } from '../stores/workspace'
import DiscordPanel from './DiscordPanel.vue'

vi.mock('../api/client', async (original) => ({
  ...(await original<typeof api>()),
  getDiscordStatus: vi.fn(),
  connectDiscord: vi.fn(),
  disconnectDiscord: vi.fn(),
  updateDiscordConsent: vi.fn(),
}))
const initial: DiscordStatus = {
  configured: true,
  connection: null,
  consent_version: '2026-11-03',
  install_url:
    'https://discord.com/oauth2/authorize?client_id=123&scope=applications.commands&integration_type=1',
}
const connected: DiscordStatus = {
  ...initial,
  connection: {
    id: 'connection-owner',
    username: 'Owner',
    timezone: 'Asia/Jakarta',
    connected_at: '2026-10-31T00:00:00Z',
    consent_version: '2026-11-03',
  },
}
const wrappers: VueWrapper[] = []
function owner(id = 'owner') {
  return { id, display_name: id, email: `${id}@example.test`, timezone: 'UTC', avatar_url: null }
}
function panel() {
  const wrapper = mount(DiscordPanel, { global: { stubs: { RouterLink: true } } })
  wrappers.push(wrapper)
  return wrapper
}
function button(wrapper: VueWrapper, text: string) {
  const result = wrapper.findAll('button').find((button) => button.text() === text)
  if (!result) throw new Error(`Missing button: ${text}`)
  return result
}

beforeEach(() => {
  vi.resetAllMocks()
  setActivePinia(createPinia())
  useWorkspaceStore().user = owner()
  vi.mocked(api.getDiscordStatus).mockResolvedValue(initial)
  vi.spyOn(window, 'confirm').mockReturnValue(true)
})
afterEach(() => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount()
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('Discord account controls', () => {
  it('requires a separate data-sharing choice and timezone before connecting', async () => {
    const wrapper = panel()
    await flushPromises()
    const connect = button(wrapper, 'Connect Discord')
    expect(connect.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain(
      'This can include tasks synchronized with Google Tasks or Calendar',
    )
    await wrapper.get('input[type="checkbox"]').setValue(true)
    await wrapper.get('input[type="text"], input:not([type])').setValue('Asia/Jakarta')
    expect(connect.attributes('disabled')).toBeUndefined()
    // Invalid response exercises the POST without attempting browser navigation.
    vi.mocked(api.connectDiscord).mockResolvedValue({ authorization_url: 'javascript:alert(1)' })
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(api.connectDiscord).toHaveBeenCalledWith(
      { timezone: 'Asia/Jakarta', consent_version: '2026-11-03', share_calendar_events: true },
      expect.any(AbortSignal),
    )
    expect(wrapper.get('[role="alert"]').text()).toContain('invalid authorization URL')
    expect(connect.attributes('disabled')).toBeDefined()
  })

  it('shows the account link and safe installation URL without exposing credentials', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue(connected)
    const wrapper = panel()
    await flushPromises()
    expect(wrapper.text()).toContain('Owner')
    expect(wrapper.text()).toContain('Asia/Jakarta')
    expect(wrapper.find('input[type="checkbox"]').exists()).toBe(false)
    expect(wrapper.get('a.secondary-button').attributes('href')).toBe(initial.install_url)
    expect(wrapper.get('a.secondary-button').attributes('rel')).toContain('noopener')
    expect(api.connectDiscord).not.toHaveBeenCalled()
  })

  it('requires separate acceptance before adding events without reconnecting', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue({
      ...connected,
      connection: { ...connected.connection!, consent_version: '2026-10-31' },
    })
    vi.mocked(api.updateDiscordConsent).mockResolvedValue(connected.connection!)
    const wrapper = panel()
    await flushPromises()
    const save = button(wrapper, 'Update Discord sharing')
    expect(save.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('Calendar events will not be shared until')
    await wrapper.get('input[type="checkbox"]').setValue(true)
    await wrapper.get('form[aria-label="Update Discord sharing"]').trigger('submit')
    await flushPromises()
    expect(api.updateDiscordConsent).toHaveBeenCalledWith(
      {
        expected_id: 'connection-owner',
        consent_version: '2026-11-03',
        share_calendar_events: true,
      },
      expect.any(AbortSignal),
    )
    expect(wrapper.text()).toContain('Calendar events are enabled')
    expect(wrapper.find('input[type="checkbox"]').exists()).toBe(false)
    expect(api.connectDiscord).not.toHaveBeenCalled()
    expect(api.disconnectDiscord).not.toHaveBeenCalled()
  })

  it('does not auto-accept a server notice that differs from the UI copy', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue({ ...initial, consent_version: '2099-01-01' })
    const wrapper = panel()
    await flushPromises()
    await wrapper.get('input[type="checkbox"]').setValue(true)
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(button(wrapper, 'Connect Discord').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('reload rakkio')
    expect(api.connectDiscord).not.toHaveBeenCalled()
  })

  it('blocks uncertain consent writes until a fresh status and unchecked choice', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue({
      ...connected,
      connection: { ...connected.connection!, consent_version: '2026-10-31' },
    })
    vi.mocked(api.updateDiscordConsent).mockRejectedValue(new Error('Request not confirmed.'))
    const wrapper = panel()
    await flushPromises()
    await wrapper.get('input[type="checkbox"]').setValue(true)
    await wrapper.get('form[aria-label="Update Discord sharing"]').trigger('submit')
    await flushPromises()
    expect(button(wrapper, 'Update Discord sharing').attributes('disabled')).toBeDefined()
    expect(api.updateDiscordConsent).toHaveBeenCalledOnce()
    await button(wrapper, 'Refresh Discord status').trigger('click')
    await flushPromises()
    expect(wrapper.get('input[type="checkbox"]').element).toHaveProperty('checked', false)
    expect(api.updateDiscordConsent).toHaveBeenCalledOnce()
  })

  it('ignores another account’s late sharing acceptance', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue({
      ...connected,
      connection: { ...connected.connection!, consent_version: '2026-10-31' },
    })
    let finish: ((value: NonNullable<DiscordStatus['connection']>) => void) | undefined
    vi.mocked(api.updateDiscordConsent).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const wrapper = panel()
    await flushPromises()
    await wrapper.get('input[type="checkbox"]').setValue(true)
    await wrapper.get('form[aria-label="Update Discord sharing"]').trigger('submit')
    vi.mocked(api.getDiscordStatus).mockResolvedValue({
      ...connected,
      connection: { ...connected.connection!, id: 'new-owner', username: 'Other' },
    })
    useWorkspaceStore().user = owner('other')
    await flushPromises()
    finish?.(connected.connection!)
    await flushPromises()
    expect(wrapper.text()).toContain('Other')
    expect(wrapper.text()).not.toContain('Calendar events are enabled')
  })

  it('confirms disconnect and sends the exact displayed connection identity', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue(connected)
    vi.mocked(api.disconnectDiscord).mockResolvedValue(undefined)
    const wrapper = panel()
    await flushPromises()
    await button(wrapper, 'Disconnect Discord').trigger('click')
    await flushPromises()
    expect(window.confirm).toHaveBeenCalledOnce()
    expect(api.disconnectDiscord).toHaveBeenCalledWith(
      { expected_id: 'connection-owner' },
      expect.any(AbortSignal),
    )
    expect(wrapper.text()).toContain('Your tasks are unchanged')
    expect(button(wrapper, 'Connect Discord').attributes('disabled')).toBeDefined()
  })

  it('does not disconnect after cancellation or silently retry uncertain writes', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue(connected)
    const wrapper = panel()
    await flushPromises()
    vi.mocked(window.confirm).mockReturnValue(false)
    await button(wrapper, 'Disconnect Discord').trigger('click')
    expect(api.disconnectDiscord).not.toHaveBeenCalled()
    vi.mocked(window.confirm).mockReturnValue(true)
    vi.mocked(api.disconnectDiscord).mockRejectedValue(new Error('Request not confirmed.'))
    await button(wrapper, 'Disconnect Discord').trigger('click')
    await flushPromises()
    expect(api.disconnectDiscord).toHaveBeenCalledOnce()
    expect(button(wrapper, 'Disconnect Discord').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[role="alert"]').text()).toContain('Refresh status before retrying')
    await button(wrapper, 'Refresh Discord status').trigger('click')
    await flushPromises()
    expect(button(wrapper, 'Disconnect Discord').attributes('disabled')).toBeUndefined()
    expect(api.disconnectDiscord).toHaveBeenCalledOnce()
  })

  it('discards another account’s late response and resets data-sharing consent', async () => {
    let finish: ((value: DiscordStatus) => void) | undefined
    vi.mocked(api.getDiscordStatus).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const wrapper = panel()
    await flushPromises()
    const oldSignal = vi.mocked(api.getDiscordStatus).mock.calls[0]?.[0]
    useWorkspaceStore().user = owner('other')
    await flushPromises()
    finish?.(connected)
    await flushPromises()
    expect(oldSignal?.aborted).toBe(true)
    expect(wrapper.text()).not.toContain('Connected as')
    expect(wrapper.get('input[type="checkbox"]').element).toHaveProperty('checked', false)
  })

  it('does not apply an old account’s completed disconnect to a new account', async () => {
    let finish: (() => void) | undefined
    vi.mocked(api.getDiscordStatus).mockResolvedValue(connected)
    vi.mocked(api.disconnectDiscord).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const wrapper = panel()
    await flushPromises()
    await button(wrapper, 'Disconnect Discord').trigger('click')
    vi.mocked(api.getDiscordStatus).mockResolvedValue({
      ...connected,
      connection: { ...connected.connection!, id: 'other-connection', username: 'Other' },
    })
    useWorkspaceStore().user = owner('other')
    await flushPromises()
    finish?.()
    await flushPromises()
    expect(wrapper.text()).toContain('Other')
    expect(wrapper.text()).not.toContain('Discord disconnected')
  })

  it('keeps disconnect available when the operator disables Discord', async () => {
    vi.mocked(api.getDiscordStatus).mockResolvedValue({ ...connected, configured: false })
    const wrapper = panel()
    await flushPromises()
    expect(wrapper.text()).toContain('Discord is disabled')
    expect(wrapper.find('a.secondary-button').exists()).toBe(false)
    expect(button(wrapper, 'Disconnect Discord').attributes('disabled')).toBeUndefined()
  })

  it('treats an unavailable status as unknown rather than connected or disabled', async () => {
    vi.mocked(api.getDiscordStatus).mockRejectedValue(new Error('Service unavailable.'))
    const wrapper = panel()
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('Refresh status')
    expect(wrapper.text()).not.toContain('Discord is not enabled')
    expect(wrapper.find('form').exists()).toBe(false)
  })

  it('aborts status requests and ignores late results after unmount', async () => {
    let finish: ((value: DiscordStatus) => void) | undefined
    vi.mocked(api.getDiscordStatus).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const wrapper = panel()
    await flushPromises()
    const signal = vi.mocked(api.getDiscordStatus).mock.calls[0]?.[0]
    wrapper.unmount()
    finish?.(connected)
    await flushPromises()
    expect(signal?.aborted).toBe(true)
  })
})
