// @vitest-environment node
import { generateKeyPairSync, sign } from 'node:crypto'
import { createRequire } from 'node:module'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { receiveDiscord, type RelayConfig } from './discord-relay.js'

const { privateKey, publicKey } = generateKeyPairSync('ed25519')
const config: RelayConfig = {
  applicationId: '123456789012345678',
  publicKey: publicKey.export({ format: 'der', type: 'spki' }).subarray(-32).toString('hex'),
  backendUrl: 'https://api.rakkio.test/webhooks/discord',
}
const payload = {
  application_id: config.applicationId,
  id: '111111111111111111',
  type: 2,
  token: 'test-interaction-token',
  user: { id: '222222222222222222' },
  data: { name: 'add', options: [{ name: 'title', value: 'Private task' }] },
}

function request(body: string, timestamp = Math.floor(Date.now() / 1000).toString()) {
  const signature = sign(null, Buffer.from(timestamp + body), privateKey).toString('hex')
  return new Request('https://rakkio.test/api/discord', {
    method: 'POST',
    headers: { 'x-signature-timestamp': timestamp, 'x-signature-ed25519': signature },
    body,
  })
}

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('Discord fast acknowledgement', () => {
  it('returns a private deferred response without waiting for a sleeping backend', async () => {
    let finish: ((response: Response) => void) | undefined
    const fetcher = vi.fn<typeof fetch>().mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const work: Promise<void>[] = []
    const raw = JSON.stringify(payload, null, 2)
    const response = await receiveDiscord(
      request(raw),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    expect(response.status).toBe(200)
    expect(response.headers.get('cache-control')).toBe('no-store')
    expect(await response.json()).toEqual({ type: 5, data: { flags: 64 } })
    expect(work).toHaveLength(1)
    expect(fetcher).toHaveBeenCalledTimes(1)
    expect(fetcher.mock.calls[0]?.[0]).toBe(config.backendUrl)
    expect(Buffer.from(fetcher.mock.calls[0]?.[1]?.body as Uint8Array).toString()).toBe(raw)
    expect(fetcher.mock.calls[0]?.[1]?.headers).toMatchObject({ 'x-rakkio-discord-deferred': '1' })
    finish?.(new Response(null, { status: 204 }))
    await Promise.all(work)
  })

  it('answers signed verification pings without backend work', async () => {
    const fetcher = vi.fn<typeof fetch>()
    const defer = vi.fn()
    const result = await receiveDiscord(request('{"type":1}'), config, defer, fetcher)
    expect(await result.json()).toEqual({ type: 1 })
    expect(defer).not.toHaveBeenCalled()
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('rejects tampering, stale signatures, wrong applications and unsupported interactions', async () => {
    const defer = vi.fn()
    const tampered = request(JSON.stringify(payload))
    const headers = new Headers(tampered.headers)
    const altered = new Request(tampered.url, {
      method: 'POST',
      headers,
      body: JSON.stringify({ ...payload, id: '999' }),
    })
    expect((await receiveDiscord(altered, config, defer)).status).toBe(401)
    expect(
      (
        await receiveDiscord(
          request(JSON.stringify(payload), String(Math.floor(Date.now() / 1000) - 301)),
          config,
          defer,
        )
      ).status,
    ).toBe(401)
    for (const changed of [
      { ...payload, application_id: '999' },
      { ...payload, type: 99 },
      { ...payload, token: '../invalid' },
    ]) {
      expect((await receiveDiscord(request(JSON.stringify(changed)), config, defer)).status).toBe(
        400,
      )
    }
    expect(defer).not.toHaveBeenCalled()
  })

  it('does not forward oversized or incomplete request bodies', async () => {
    const defer = vi.fn()
    expect((await receiveDiscord(request(' '.repeat(65537)), config, defer)).status).toBe(413)
    vi.useFakeTimers()
    const signed = request(JSON.stringify(payload))
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(Buffer.from(JSON.stringify(payload)))
      },
    })
    const partial = new Request(signed.url, {
      method: 'POST',
      headers: signed.headers,
      body: stream,
      duplex: 'half',
    } as RequestInit)
    const pending = receiveDiscord(partial, config, defer)
    await vi.advanceTimersByTimeAsync(1001)
    expect((await pending).status).toBe(413)
    expect(defer).not.toHaveBeenCalled()
  })

  it('retries forwarding with the same signed bytes rather than inventing another command', async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockRejectedValueOnce(new Error('timeout'))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls[0]?.[1]?.body).toEqual(fetcher.mock.calls[1]?.[1]?.body)
    expect(fetcher.mock.calls[0]?.[1]?.headers).toEqual(fetcher.mock.calls[1]?.[1]?.headers)
  })

  it('delivers the committed backend reply from the relay rather than asking Render to contact Discord', async () => {
    const message = {
      content: 'Today: private task',
      components: [],
      allowed_mentions: { parse: ['everyone'] },
    }
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(Response.json(message))
      .mockResolvedValueOnce(new Response(null, { status: 200 }))
    const work: Promise<void>[] = []
    const ack = await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    expect(await ack.json()).toEqual({ type: 5, data: { flags: 64 } })
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls[1]?.[1]?.method).toBe('PATCH')
    expect(JSON.parse(String(fetcher.mock.calls[1]?.[1]?.body))).toEqual({
      ...message,
      allowed_mentions: { parse: [] },
    })
  })

  it('retries only delivery, not the committed task operation, after a short provider cooldown', async () => {
    vi.useFakeTimers()
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(Response.json({ content: 'Added to Inbox', components: [] }))
      .mockResolvedValueOnce(Response.json({ retry_after: 1 }, { status: 429 }))
      .mockResolvedValueOnce(new Response(null, { status: 200 }))
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await vi.advanceTimersByTimeAsync(0)
    expect(fetcher).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(1000)
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(3)
    expect(fetcher.mock.calls.filter(([url]) => url === config.backendUrl)).toHaveLength(1)
    expect(fetcher.mock.calls[1]?.[1]?.body).toEqual(fetcher.mock.calls[2]?.[1]?.body)
  })

  it('does not replay tasks or make an early fallback after a long direct-provider limit', async () => {
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(Response.json({ content: 'private-reply-content', components: [] }))
      .mockResolvedValueOnce(
        Response.json(
          { retry_after: 3317, global: true, message: 'private-provider-detail' },
          { status: 429 },
        ),
      )
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls.filter(([url]) => url === config.backendUrl)).toHaveLength(1)
    const logs = JSON.stringify(warning.mock.calls)
    expect(logs).toContain('3317')
    for (const secret of [
      'private-reply-content',
      'private-provider-detail',
      payload.token,
      'webhooks/',
    ])
      expect(logs).not.toContain(secret)
  })

  it('uses the later fractional provider hint rather than retrying early', async () => {
    vi.useFakeTimers()
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(Response.json({ content: 'Plan', components: [] }))
      .mockResolvedValueOnce(
        Response.json({ retry_after: 4.1 }, { status: 429, headers: { 'retry-after': '1' } }),
      )
      .mockResolvedValueOnce(new Response(null, { status: 200 }))
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await vi.advanceTimersByTimeAsync(4999)
    expect(fetcher).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(1)
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(3)
  })

  it('bounds and validates backend data before exposing only an uncertain notice', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    for (const body of [
      'x'.repeat(8193),
      JSON.stringify({ content: 'x'.repeat(2001), components: [] }),
      JSON.stringify({ content: 'private-bad-reply', components: {} }),
    ]) {
      const fetcher = vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(new Response(body, { status: 200 }))
        .mockResolvedValueOnce(new Response(null, { status: 200 }))
      const work: Promise<void>[] = []
      await receiveDiscord(
        request(JSON.stringify(payload)),
        config,
        (pending) => work.push(pending),
        fetcher,
      )
      await Promise.all(work)
      expect(fetcher).toHaveBeenCalledTimes(2)
      const reply = JSON.parse(String(fetcher.mock.calls[1]?.[1]?.body))
      expect(reply.content).toContain('may already have succeeded')
      expect(reply.content).not.toContain('private-bad-reply')
    }
  })

  it('preserves private menu components but ignores unrelated backend fields', async () => {
    const components = [
      {
        type: 1,
        components: [
          { type: 3, custom_id: 'test-menu', options: [{ label: 'Task', value: 'opaque-value' }] },
        ],
      },
    ]
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(
        Response.json({
          content: 'Choose a task',
          components,
          unexpected: 'private-extra',
          flags: 0,
        }),
      )
      .mockResolvedValueOnce(new Response(null, { status: 200 }))
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await Promise.all(work)
    expect(JSON.parse(String(fetcher.mock.calls[1]?.[1]?.body))).toEqual({
      content: 'Choose a task',
      components,
      allowed_mentions: { parse: [] },
    })
  })

  it('waits for a provider cooldown before replaying the same receipt', async () => {
    vi.useFakeTimers()
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(
        new Response(null, {
          status: 429,
          headers: {
            'retry-after': '1',
            'x-rakkio-discord-provider-rate-limit': '1',
          },
        }),
      )
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await vi.advanceTimersByTimeAsync(0)
    expect(fetcher).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(999)
    expect(fetcher).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls[0]?.[1]?.body).toEqual(fetcher.mock.calls[1]?.[1]?.body)
  })

  it('does not retry or send another provider message before a long or unknown cooldown', async () => {
    for (const delay of [null, '60', '-1', 'NaN']) {
      const headers: Record<string, string> = { 'x-rakkio-discord-provider-rate-limit': '1' }
      if (delay !== null) headers['retry-after'] = delay
      const fetcher = vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(new Response(null, { status: 429, headers }))
        .mockResolvedValueOnce(new Response(null, { status: 204 }))
      const work: Promise<void>[] = []
      await receiveDiscord(
        request(JSON.stringify(payload)),
        config,
        (pending) => work.push(pending),
        fetcher,
      )
      await Promise.all(work)
      expect(fetcher).toHaveBeenCalledTimes(1)
    }
  })

  it('caps total cooldown time and does not fall back into an exhausted provider', async () => {
    vi.useFakeTimers()
    const limited = () =>
      new Response(null, {
        status: 429,
        headers: {
          'retry-after': '3',
          'x-rakkio-discord-provider-rate-limit': '1',
        },
      })
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(limited())
      .mockResolvedValueOnce(limited())
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await vi.advanceTimersByTimeAsync(3_000)
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(2)
  })

  it('does not immediately replay an unknown backend cooldown and keeps the uncertain notice', async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(null, { status: 429 }))
      .mockResolvedValueOnce(new Response(null, { status: 200 }))
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(2)
    expect(fetcher.mock.calls[1]?.[1]?.method).toBe('PATCH')
    expect(String(fetcher.mock.calls[1]?.[1]?.body)).toContain('may already have succeeded')
  })

  it('reports an uncertain outcome privately after delivery failure', async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(null, { status: 503 }))
      .mockResolvedValueOnce(new Response(null, { status: 503 }))
      .mockResolvedValueOnce(new Response(null, { status: 200 }))
    const work: Promise<void>[] = []
    await receiveDiscord(
      request(JSON.stringify(payload)),
      config,
      (pending) => work.push(pending),
      fetcher,
    )
    await Promise.all(work)
    expect(fetcher).toHaveBeenCalledTimes(3)
    const update = fetcher.mock.calls[2]
    expect(update?.[0]).toBe(
      `https://discord.com/api/v10/webhooks/${config.applicationId}/${payload.token}/messages/@original`,
    )
    expect(update?.[1]?.headers).toMatchObject({
      'User-Agent': 'DiscordBot (https://github.com/danarrigo/rakkio, 0.1.0)',
    })
    const message = JSON.parse(String(update?.[1]?.body))
    expect(message.content).toContain('may already have succeeded')
    expect(message.allowed_mentions).toEqual({ parse: [] })
  })

  it('fails closed when disabled, misconfigured, or called with another method', async () => {
    const defer = vi.fn()
    for (const backendUrl of [
      'http://api.rakkio.test/webhooks/discord',
      'https://api.rakkio.test/other',
      'https://user:secret@api.rakkio.test/webhooks/discord',
    ]) {
      expect((await receiveDiscord(request('{}'), { ...config, backendUrl }, defer)).status).toBe(
        503,
      )
    }
    expect(
      (await receiveDiscord(new Request('https://rakkio.test/api/discord'), config, defer)).status,
    ).toBe(405)
    expect(defer).not.toHaveBeenCalled()
  })
})

it('keeps Redocly’s callable minimatch interface while using the patched named API', () => {
  const require = createRequire(import.meta.url)
  const legacy = require('@rakkio/minimatch-compat')
  expect(typeof legacy).toBe('function')
  expect(legacy('example.com/path', '*.com/**')).toBe(true)
  expect(new legacy.Minimatch('*.com').match('example.com')).toBe(true)
})
