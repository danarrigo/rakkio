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
  // commits its receipt and task changes atomically before returning reply data.
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

interface Reply {
  content: string
  components: unknown[]
  allowed_mentions: { parse: never[] }
}

const UNCERTAIN_REPLY: Reply = {
  content:
    'Could not confirm this command. Check rakkio before submitting it again; it may already have succeeded.',
  components: [],
  allowed_mentions: { parse: [] },
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

async function responseJson(response: Response, limit: number): Promise<unknown> {
  const reader = response.body?.getReader()
  if (!reader) return null
  const chunks: Uint8Array[] = []
  let length = 0
  let ended = false
  try {
    while (true) {
      const part = await reader.read()
      if (part.done) {
        ended = true
        break
      }
      length += part.value.byteLength
      if (length > limit) return null
      chunks.push(part.value)
    }
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)))
  } catch {
    return null
  } finally {
    if (!ended) await reader.cancel().catch(() => undefined)
    reader.releaseLock()
  }
}

function cooldown(response: Response, body?: unknown): number | undefined {
  const header = response.headers.get('retry-after')?.trim() ?? ''
  let seconds: number | undefined
  if (/^\d+(?:\.\d+)?$/.test(header)) seconds = Number(header)
  else if (/^[A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} GMT$/.test(header))
    seconds = Math.max(0, (Date.parse(header) - Date.now()) / 1000)
  const candidates = [seconds, record(body) ? body.retry_after : undefined].filter(
    (value): value is number => typeof value === 'number' && Number.isFinite(value) && value >= 0,
  )
  if (!candidates.length) return undefined
  // Validate only after choosing the later hint: never shorten a long cooldown.
  const delay = Math.ceil(Math.max(...candidates))
  return delay <= 86_400 ? delay : undefined
}

async function waitForCooldown(
  seconds: number | undefined,
  budget: { ms: number },
): Promise<boolean> {
  if (seconds === undefined || seconds * 1000 > budget.ms) return false
  budget.ms -= seconds * 1000
  await new Promise<void>((resolve) => setTimeout(resolve, seconds * 1000))
  return true
}

async function deliver(
  reply: Reply,
  application: string,
  token: string,
  fetcher: typeof fetch,
  budget: { ms: number },
  attempts: number,
): Promise<'done' | 'stopped' | 'failed'> {
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const result = await fetcher(
        `${DISCORD}/webhooks/${application}/${token}/messages/@original`,
        {
          method: 'PATCH',
          headers: {
            'Content-Type': 'application/json',
            'User-Agent': 'DiscordBot (https://github.com/danarrigo/rakkio, 0.1.0)',
          },
          body: JSON.stringify(reply),
          redirect: 'error',
          signal: AbortSignal.timeout(10000),
        },
      )
      if (result.status === 429) {
        const body = await responseJson(result, BODY_LIMIT)
        const delay = cooldown(result, body)
        const hint = result.headers.get('x-ratelimit-scope')
        const scope =
          hint === 'global' || hint === 'user' || hint === 'shared'
            ? hint
            : result.headers.get('x-ratelimit-global') === 'true' ||
                (record(body) && body.global === true)
              ? 'global'
              : 'unavailable'
        // Only fixed labels, status and validated hints. Never stringify a
        // fetch error, body, task content or a token-bearing webhook URL.
        console.warn('Discord relay delivery rate limited', {
          status: 429,
          scope,
          retry_after_seconds: delay ?? null,
        })
        if (attempt + 1 < attempts && (await waitForCooldown(delay, budget))) continue
        return 'stopped'
      }
      await result.body?.cancel()
      if (result.ok) return 'done'
      console.warn('Discord relay delivery rejected', { status: result.status })
      return [401, 403, 404].includes(result.status) ? 'stopped' : 'failed'
    } catch {
      console.warn('Discord relay delivery unavailable')
      return 'failed'
    }
  }
  return 'failed'
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
  // Two 40s backend calls + two 10s delivery attempts + one 10s notice +
  // at most 5s cooldown leave room for the 1s reader within the 120s budget.
  const budget = { ms: 5000 }
  let reply: Reply | null = null
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
      if (result.status === 200) {
        const value = await responseJson(result, 8192)
        if (
          record(value) &&
          typeof value.content === 'string' &&
          value.content.length > 0 &&
          value.content.length <= 2000 &&
          Array.isArray(value.components)
        ) {
          // Only the supported message fields; mentions remain disabled even if
          // a future backend accidentally sends a broader allowed_mentions.
          reply = {
            content: value.content,
            components: value.components,
            allowed_mentions: { parse: [] },
          }
        } else console.warn('Discord relay backend reply invalid')
        break
      }
      await result.body?.cancel()
      if (result.status === 204) return // Older API already delivered: rolling-deploy compatibility.
      if (result.status === 429) {
        const providerLimited = result.headers.get('x-rakkio-discord-provider-rate-limit') === '1'
        if (await waitForCooldown(cooldown(result), budget)) continue
        if (providerLimited) return // Legacy API: do not issue an early fallback.
        break
      }
      if (result.status < 500) {
        console.warn('Discord relay backend rejected', { status: result.status })
        break
      }
    } catch {
      // Same signed bytes/ID are retried; never rerun a task after receiving its result.
      console.warn('Discord relay backend unavailable')
    }
  }
  const outcome = await deliver(
    reply ?? UNCERTAIN_REPLY,
    application,
    token,
    fetcher,
    budget,
    reply ? 2 : 1,
  )
  if (reply && outcome === 'failed')
    await deliver(UNCERTAIN_REPLY, application, token, fetcher, budget, 1)
}
