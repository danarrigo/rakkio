import { createPublicKey, verify } from 'node:crypto'

const BODY_LIMIT = 64 * 1024
const DISCORD = 'https://discord.com/api/v10'

export interface RelayConfig {
  applicationId: string
  publicKey: string
  backendUrl: string
}

/** The relay has no database access, OAuth secret, or bot token. */
export async function receiveDiscord(
  request: Request,
  config: RelayConfig,
  defer: (work: Promise<void>) => void,
  fetcher: typeof fetch = fetch,
): Promise<Response> {
  const headers = { 'Cache-Control': 'no-store' }
  if (request.method !== 'POST')
    return new Response(null, { status: 405, headers: { ...headers, Allow: 'POST' } })
  let backend: URL
  try {
    backend = new URL(config.backendUrl)
    if (
      !/^\d{1,20}$/.test(config.applicationId) ||
      !/^[a-f\d]{64}$/i.test(config.publicKey) ||
      backend.protocol !== 'https:' ||
      backend.username ||
      backend.password ||
      backend.search ||
      backend.hash ||
      backend.pathname !== '/webhooks/discord'
    )
      throw new Error('Invalid relay configuration')
  } catch {
    return new Response('Discord integration is not configured.', { status: 503, headers })
  }
  const timestamp = request.headers.get('x-signature-timestamp') ?? ''
  const signature = request.headers.get('x-signature-ed25519') ?? ''
  if (
    !/^\d{1,12}$/.test(timestamp) ||
    Math.abs(Date.now() / 1000 - Number(timestamp)) > 300 ||
    !/^[a-f\d]{128}$/i.test(signature)
  )
    return new Response(null, { status: 401, headers })

  let raw: Uint8Array
  try {
    raw = await boundedBody(request)
  } catch {
    return new Response(null, { status: 413, headers })
  }
  const key = createPublicKey({
    key: Buffer.concat([
      Buffer.from('302a300506032b6570032100', 'hex'),
      Buffer.from(config.publicKey, 'hex'),
    ]),
    format: 'der',
    type: 'spki',
  })
  if (
    !verify(null, Buffer.concat([Buffer.from(timestamp), raw]), key, Buffer.from(signature, 'hex'))
  )
    return new Response(null, { status: 401, headers })

  let interaction: { type?: number; application_id?: string; id?: string; token?: string }
  try {
    interaction = JSON.parse(Buffer.from(raw).toString('utf8'))
    if (!interaction || typeof interaction !== 'object') throw new Error('Invalid interaction')
  } catch {
    return new Response(null, { status: 400, headers })
  }
  if (interaction.type === 1) return Response.json({ type: 1 }, { headers })
  if (
    ![2, 3].includes(interaction.type ?? 0) ||
    interaction.application_id !== config.applicationId ||
    typeof interaction.id !== 'string' ||
    !/^\d{1,20}$/.test(interaction.id) ||
    typeof interaction.token !== 'string' ||
    !/^[A-Za-z0-9_.-]{1,4096}$/.test(interaction.token)
  )
    return new Response(null, { status: 400, headers })

  // Forwarding retries keep the original signed interaction ID. The backend
  // commits its receipt and task changes atomically before replying to Discord.
  defer(
    forward(
      backend.href,
      raw,
      timestamp,
      signature,
      config.applicationId,
      interaction.token,
      fetcher,
    ),
  )
  return Response.json({ type: 5, data: { flags: 64 } }, { headers })
}

async function boundedBody(request: Request): Promise<Uint8Array> {
  const reader = request.body?.getReader()
  if (!reader) throw new Error('Missing body')
  const chunks: Uint8Array[] = []
  let length = 0
  let timedOut = false
  const timeout = setTimeout(() => {
    timedOut = true
    void reader.cancel().catch(() => undefined)
  }, 1000)
  let ended = false
  try {
    while (true) {
      const part = await reader.read()
      if (part.done) {
        ended = true
        break
      }
      length += part.value.byteLength
      if (length > BODY_LIMIT) throw new Error('Body too large')
      chunks.push(part.value)
    }
    if (timedOut || length === 0) throw new Error('Incomplete body')
    return Buffer.concat(chunks)
  } finally {
    clearTimeout(timeout)
    if (!ended) await reader.cancel().catch(() => undefined)
    reader.releaseLock()
  }
}

async function forward(
  backend: string,
  raw: Uint8Array,
  timestamp: string,
  signature: string,
  application: string,
  token: string,
  fetcher: typeof fetch,
): Promise<void> {
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const result = await fetcher(backend, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'x-signature-timestamp': timestamp,
          'x-signature-ed25519': signature,
          'x-rakkio-discord-deferred': '1',
        },
        body: Buffer.from(raw),
        redirect: 'error',
        signal: AbortSignal.timeout(40000),
      })
      await result.body?.cancel()
      if (result.status === 204) return
      if (result.status < 500 && result.status !== 429) break
    } catch {
      // Never log request bodies, signed headers, interaction tokens, or URLs.
    }
  }
  try {
    const result = await fetcher(`${DISCORD}/webhooks/${application}/${token}/messages/@original`, {
      method: 'PATCH',
      headers: {
        'Content-Type': 'application/json',
        'User-Agent': 'DiscordBot (https://github.com/danarrigo/rakkio, 0.1.0)',
      },
      body: JSON.stringify({
        content:
          'Could not confirm this command. Check rakkio before submitting it again; it may already have succeeded.',
        components: [],
        allowed_mentions: { parse: [] },
      }),
      redirect: 'error',
      signal: AbortSignal.timeout(10000),
    })
    await result.body?.cancel()
  } catch {
    // The platform may be unavailable too; no success is invented.
  }
}
