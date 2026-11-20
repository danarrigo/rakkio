import { describe, expect, it } from 'vitest'
import {
  EdunexRejectedError,
  EdunexUnavailableError,
  fetchAssignments,
  normalizeToken,
  toAssignment,
  tokenExpiry,
} from './edunex'

function resource(id: number, attributes: Record<string, unknown> = {}) {
  return {
    type: 'tasks',
    id: String(id),
    attributes: {
      name: `Task ${id}`,
      course_code: 'IF2110',
      course_name: 'Algoritma',
      end_at: '2026-11-20T23:59:00+07:00',
      answers: [],
      ...attributes,
    },
  }
}

describe('Edunex assignments', () => {
  it('normalizes titles, deadlines and submission state', () => {
    expect(toAssignment(resource(1, { name: ' Tugas 1 ', answers: [{ is_sent: 1 }] }))).toEqual({
      id: '1',
      title: '[IF2110] Tugas 1',
      course_name: 'Algoritma',
      due_at: '2026-11-20T16:59:00.000Z',
      submitted: true,
    })
    expect(toAssignment(resource(2, { answers: [{ is_sent: 0 }] }))?.submitted).toBe(false)
    expect(toAssignment(resource(3, { course_code: null }))?.title).toBe('Task 3')
  })

  it('skips entries that cannot become tasks instead of failing the sync', () => {
    expect(toAssignment(resource(4, { end_at: null }))).toBeNull()
    expect(toAssignment(resource(5, { name: null, course_code: '' }))).toBeNull()
    expect(toAssignment(resource(6, { answers: null }))?.submitted).toBe(false)
    expect(toAssignment(null)).toBeNull()
  })

  it('follows offsets until the reported total and sends only the bearer token', async () => {
    const requests: URL[] = []
    const fetcher = (async (input: URL, init?: RequestInit) => {
      requests.push(input)
      expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer fixture')
      expect(init?.credentials).toBe('omit')
      const offset = Number(input.searchParams.get('page[offset]'))
      const data = Array.from({ length: Math.max(0, Math.min(10, 23 - offset)) }, (_, index) =>
        resource(offset + index),
      )
      return new Response(JSON.stringify({ meta: { total: 23 }, data }))
    }) as typeof fetch
    const assignments = await fetchAssignments('fixture', fetcher)
    expect(assignments).toHaveLength(23)
    expect(requests.map((url) => url.searchParams.get('page[offset]'))).toEqual(['0', '10', '20'])
  })

  it('separates a rejected token from an outage', async () => {
    const status = (code: number) =>
      (async () => new Response('{}', { status: code })) as typeof fetch
    await expect(fetchAssignments('x', status(401))).rejects.toBeInstanceOf(EdunexRejectedError)
    await expect(fetchAssignments('x', status(502))).rejects.toBeInstanceOf(EdunexUnavailableError)
    const offline = (async () => {
      throw new TypeError('Failed to fetch')
    }) as typeof fetch
    await expect(fetchAssignments('x', offline)).rejects.toBeInstanceOf(EdunexUnavailableError)
  })

  it('accepts pasted tokens with or without the Bearer prefix and reads expiry', () => {
    expect(normalizeToken('  Bearer abc.def.ghi ')).toBe('abc.def.ghi')
    expect(normalizeToken('abc')).toBe('abc')
    const payload = btoa(JSON.stringify({ exp: 1795142545 })).replace(/=+$/, '')
    expect(tokenExpiry(`h.${payload}.s`)?.toISOString()).toBe('2026-11-20T02:42:25.000Z')
    expect(tokenExpiry('opaque')).toBeNull()
  })
})
