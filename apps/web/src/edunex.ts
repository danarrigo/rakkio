import * as api from './api/client'
import type { EdunexAssignment, EdunexSyncReport } from './api/types'

// Edunex refuses connections from hosted servers, so the owner's browser
// fetches assignments directly. The token stays in this browser only.
const API_BASE = 'https://api-edunex.cognisia.id/'
const TOKEN_KEY = 'rakkio.edunex-token'
// Matches the Edunex web app; larger pages are not guaranteed.
const PAGE_SIZE = 10
const MAX_PAGES = 200
const AUTO_SYNC_AFTER_MS = 3 * 60 * 60 * 1000

export class EdunexRejectedError extends Error {
  constructor() {
    super('Edunex rejected the token. Copy a fresh one from Edunex.')
    this.name = 'EdunexRejectedError'
  }
}

export class EdunexUnavailableError extends Error {
  constructor(detail?: string) {
    super(`Edunex could not be reached${detail ? ` (${detail})` : ''}. Try again later.`)
    this.name = 'EdunexUnavailableError'
  }
}

export function normalizeToken(value: string) {
  return value
    .trim()
    .replace(/^bearer\s+/i, '')
    .trim()
}

export function readToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY)
  } catch {
    return null
  }
}

export function saveToken(token: string) {
  localStorage.setItem(TOKEN_KEY, token)
}

export function forgetToken() {
  try {
    localStorage.removeItem(TOKEN_KEY)
  } catch {
    /* Storage may be unavailable; nothing to remove then. */
  }
}

/** Reads a JWT's expiry without verifying it; for display only. */
export function tokenExpiry(token: string): Date | null {
  const payload = token.split('.')[1]
  if (!payload) return null
  try {
    const json = atob(payload.replace(/-/g, '+').replace(/_/g, '/'))
    const exp = (JSON.parse(json) as { exp?: unknown }).exp
    return typeof exp === 'number' ? new Date(exp * 1000) : null
  } catch {
    return null
  }
}

function truthy(value: unknown) {
  if (typeof value === 'boolean') return value
  if (typeof value === 'number') return value !== 0
  if (typeof value === 'string') return !['', '0', 'false'].includes(value.trim())
  return false
}

/** Converts one Edunex JSON:API task; skips entries that cannot be imported. */
export function toAssignment(resource: unknown): EdunexAssignment | null {
  if (!resource || typeof resource !== 'object') return null
  const { id, attributes } = resource as { id?: unknown; attributes?: Record<string, unknown> }
  const taskId = typeof id === 'number' ? String(id) : typeof id === 'string' ? id : ''
  if (!taskId || taskId.length > 64 || !attributes) return null
  const due = typeof attributes.end_at === 'string' ? new Date(attributes.end_at) : null
  if (!due || Number.isNaN(due.getTime())) return null
  const name = typeof attributes.name === 'string' ? attributes.name.trim() : ''
  const code = typeof attributes.course_code === 'string' ? attributes.course_code.trim() : ''
  const title = [...(code ? `[${code}] ${name}` : name)].slice(0, 240).join('').trim()
  if (!title) return null
  const courseName =
    typeof attributes.course_name === 'string'
      ? [...attributes.course_name.trim()].slice(0, 500).join('')
      : ''
  const answers = Array.isArray(attributes.answers) ? attributes.answers : []
  return {
    id: taskId,
    title,
    course_name: courseName,
    due_at: due.toISOString(),
    submitted: answers.some(
      (answer) => !!answer && typeof answer === 'object' && truthy(answer.is_sent),
    ),
  }
}

/** Every active assignment visible to the token's owner. */
export async function fetchAssignments(
  token: string,
  fetcher: typeof fetch = fetch,
): Promise<EdunexAssignment[]> {
  const assignments: EdunexAssignment[] = []
  let offset = 0
  for (let page = 0; page < MAX_PAGES; page++) {
    const url = new URL('course/tasks', API_BASE)
    url.searchParams.set('filter[is_active][is]', '1')
    url.searchParams.set('sort', '-start_at')
    url.searchParams.set('page[limit]', String(PAGE_SIZE))
    url.searchParams.set('page[offset]', String(offset))
    let response: Response
    try {
      response = await fetcher(url, {
        headers: { Authorization: `Bearer ${token}`, Accept: 'application/vnd.api+json' },
        credentials: 'omit',
        referrerPolicy: 'no-referrer',
      })
    } catch {
      throw new EdunexUnavailableError()
    }
    if (response.status === 401 || response.status === 403) throw new EdunexRejectedError()
    if (!response.ok) throw new EdunexUnavailableError(`HTTP ${response.status}`)
    let body: { data?: unknown; meta?: { total?: unknown } }
    try {
      body = await response.json()
    } catch {
      throw new EdunexUnavailableError('unexpected response')
    }
    const data = Array.isArray(body.data) ? body.data : []
    for (const resource of data) {
      const assignment = toAssignment(resource)
      if (assignment) assignments.push(assignment)
    }
    offset += data.length
    const total = Number(body.meta?.total)
    const finished = Number.isFinite(total) ? offset >= total : data.length < PAGE_SIZE
    if (!data.length || finished) break
  }
  return assignments
}

export async function synchronize(token: string): Promise<EdunexSyncReport> {
  return api.importEdunexAssignments(await fetchAssignments(token))
}

/** Syncs quietly when rakkio opens, at most every few hours. */
export async function autoSynchronize(): Promise<EdunexSyncReport | null> {
  const token = readToken()
  if (!token) return null
  const status = await api.getEdunexStatus()
  const last = status.last_synced_at ? Date.parse(status.last_synced_at) : 0
  if (Date.now() - last < AUTO_SYNC_AFTER_MS) return null
  return synchronize(token)
}
