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

afterEach(() => vi.useRealTimers())

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
