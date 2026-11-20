import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  type Batch,
  ENDPOINT,
  type ErrorEventBody,
  FLUSH_INTERVAL_MS,
  MAX_BATCH_BYTES,
  MAX_DISTINCT_ERRORS,
  Reporter,
  type ReporterOptions,
  beaconSend,
  fit,
  pageOf,
} from './reporter'

function setup(overrides: Partial<ReporterOptions> = {}) {
  const sent: Batch[] = []
  /** `anonymous` for each batch sent, in order. */
  const anonymous: boolean[] = []
  let signedIn = false
  let ids = 0
  const timers: Array<() => void> = []
  const reporter = new Reporter({
    release: 'abc123',
    pathname: () => '/login',
    isSignedIn: () => signedIn,
    send: (body, options) => {
      sent.push(JSON.parse(body) as Batch)
      anonymous.push(options.anonymous)
    },
    newPageLoadId: () => String(++ids).padStart(32, '0'),
    setTimer: (fn) => timers.push(fn),
    clearTimer: () => {},
    ...overrides,
  })
  return {
    reporter,
    sent,
    anonymous,
    timers,
    signIn: (value: boolean) => {
      signedIn = value
    },
    /** The error events sent, in order. */
    events: () =>
      sent.flatMap((b) => b.events).filter((e): e is ErrorEventBody => e.type === 'error'),
    /** `page` of each page load sent, in order. */
    loads: () =>
      sent.flatMap((b) => b.events.filter((e) => e.type === 'page_load').map(() => b.page)),
  }
}

const boom = (message = 'TypeError: boom', stack?: string) => ({
  kind: 'error' as const,
  message,
  stack: stack ?? `${message}\n    at f (https://overkill.example/_astro/a.js:1:2)`,
})

describe('Reporter', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('sends one event with a count for an error repeated 50 times', () => {
    const { reporter, events, sent } = setup()
    for (let i = 0; i < 50; i++) reporter.capture(boom())
    reporter.flush()
    expect(events()).toHaveLength(1)
    expect(events()[0]).toMatchObject({ type: 'error', kind: 'error', count: 50 })
    expect(sent[0]).toMatchObject({ release: 'abc123', page: '/login' })
    expect(sent[0].page_load_id).toMatch(/^[0-9a-f]{32}$/)
  })

  it('reports at most 20 distinct errors per page load, still counting repeats', () => {
    const { reporter, events } = setup()
    for (let i = 0; i < 500; i++) reporter.capture(boom(`Error: e${i}`))
    reporter.capture(boom('Error: e0'))
    reporter.flush()
    expect(events()).toHaveLength(MAX_DISTINCT_ERRORS)
    expect(events()[0].count).toBe(2)
  })

  it('keeps the cap across flushes, and sends a repeat after a flush again', () => {
    const { reporter, events } = setup()
    for (let i = 0; i < 20; i++) reporter.capture(boom(`Error: e${i}`))
    reporter.flush()
    reporter.capture(boom('Error: new'))
    reporter.capture(boom('Error: e3'))
    reporter.flush()
    expect(events().map((e) => e.message)).toContain('Error: e3')
    expect(events().map((e) => e.message)).not.toContain('Error: new')
  })

  it('sends a page load for each call, on the page it happened, never merged', () => {
    let path = '/dashboard'
    const { reporter, loads, events } = setup({ pathname: () => path })
    reporter.pageLoad()
    path = '/dashboard/settings/'
    reporter.pageLoad()
    reporter.pageLoad()
    reporter.flush()
    expect(loads()).toEqual(['/dashboard', '/dashboard/settings', '/dashboard/settings'])
    expect(events()).toEqual([])
  })

  it('does not count page loads against the distinct error cap', () => {
    const { reporter, events, loads } = setup()
    for (let i = 0; i < 40; i++) reporter.pageLoad()
    for (let i = 0; i < MAX_DISTINCT_ERRORS; i++) reporter.capture(boom(`TypeError: e${i}`))
    reporter.flush()
    expect(loads()).toHaveLength(40)
    expect(events()).toHaveLength(MAX_DISTINCT_ERRORS)
  })

  it('schedules a flush for a page load and stays silent if the page throws', () => {
    const { reporter, timers, sent } = setup({
      pathname: () => {
        throw new Error('no location')
      },
    })
    expect(() => reporter.pageLoad()).not.toThrow()
    expect(timers).toHaveLength(0)
    const ok = setup()
    ok.reporter.pageLoad()
    expect(ok.timers).toHaveLength(1)
    ok.timers[0]()
    expect(ok.loads()).toEqual(['/login'])
    expect(sent).toEqual([])
  })

  it('sends an API failure with its route, status and a well-formed trace ID only', () => {
    const { reporter, events } = setup()
    const trace = 'abcdef0123456789abcdef0123456789'
    reporter.capture({
      kind: 'api',
      message: 'PATCH /users/me failed: status 500',
      apiRoute: '/users/me?email=a@b.c',
      status: 500,
      traceId: trace,
    })
    reporter.capture({
      kind: 'api',
      message: 'GET /x failed: status 502',
      apiRoute: '/x',
      status: 502,
      traceId: 'not-a-trace-id',
    })
    reporter.flush()
    expect(events()[0]).toMatchObject({ api_route: '/users/me', status: 500, trace_id: trace })
    expect(events()[1]).not.toHaveProperty('trace_id')
  })

  it('holds timed flushes while the server is unreachable, then sends on its return', () => {
    const { reporter, timers, events } = setup()
    reporter.markOffline()
    reporter.capture({
      kind: 'api',
      message: 'POST /auth/register failed: network error',
      apiRoute: '/auth/register',
      networkFailed: true,
    })
    timers.shift()?.()
    expect(events()).toEqual([])
    expect(timers).toHaveLength(1) // rescheduled, not dropped
    reporter.markOnline()
    expect(events()).toHaveLength(1)
    expect(events()[0]).toMatchObject({ network_failed: true, api_route: '/auth/register' })
    expect(events()[0]).not.toHaveProperty('status')
  })

  it('still sends held reports when the page is hidden', () => {
    const { reporter, events } = setup()
    reporter.markOffline()
    reporter.capture(boom())
    reporter.flush() // visibilitychange: hidden, pagehide
    expect(events()).toHaveLength(1)
  })

  it('sends the island of a boundary error, and keeps islands apart', () => {
    const { reporter, events } = setup()
    reporter.capture({ ...boom(), kind: 'boundary', island: 'SettingsPanel' })
    reporter.capture({ ...boom(), kind: 'boundary', island: 'SettingsPanel' })
    reporter.capture({ ...boom(), kind: 'boundary', island: 'DashboardHeader' })
    reporter.flush()
    expect(events().map((e) => [e.island, e.count])).toEqual([
      ['SettingsPanel', 2],
      ['DashboardHeader', 1],
    ])
  })

  it('scrubs before queueing', () => {
    const { reporter, events } = setup()
    reporter.capture(
      boom(
        'Error: at https://overkill.example/verify-magic#token=abc for a@b.io',
        'Error\n    at f (https://overkill.example/reset-password?email=a%40b.io:1:2)'
      )
    )
    reporter.flush()
    const body = JSON.stringify(events())
    expect(body).not.toContain('abc')
    expect(body).not.toContain('a@b.io')
    expect(body).not.toContain('a%40b.io')
    expect(events()[0].message).toBe('Error: at https://overkill.example/verify-magic for [EMAIL]')
  })

  it('starts a new page-load ID when the Member signs in or out', () => {
    const { reporter, sent, signIn } = setup()
    reporter.capture(boom('Error: before'))
    signIn(true)
    reporter.capture(boom('Error: signed in'))
    signIn(false)
    reporter.capture(boom('Error: signed out'))
    reporter.flush()
    const ids = sent.map((b) => b.page_load_id)
    expect(ids).toHaveLength(3)
    expect(new Set(ids).size).toBe(3)
  })

  it('keeps one page-load ID while the session does not change', () => {
    const { reporter, sent } = setup()
    reporter.capture(boom('Error: one'))
    reporter.flush()
    reporter.capture(boom('Error: two'))
    reporter.flush()
    expect(sent[0].page_load_id).toBe(sent[1].page_load_id)
  })

  it('groups by page and splits into batches of at most 30 events', () => {
    let path = '/login'
    const { reporter, sent } = setup({ pathname: () => path })
    for (let i = 0; i < 15; i++) reporter.capture(boom(`Error: l${i}`))
    path = '/register/'
    for (let i = 0; i < 5; i++) reporter.capture(boom(`Error: r${i}`))
    reporter.flush()
    expect(sent.map((b) => [b.page, b.events.length])).toEqual([
      ['/login', 15],
      ['/register', 5],
    ])
  })

  it('splits a batch over 16 KB and trims a stack that is still too big', () => {
    const big = Array.from({ length: 6 }, (_, i) => ({
      type: 'error' as const,
      kind: 'error' as const,
      message: `Error: ${i}`,
      stack: '😀'.repeat(4000),
      count: 1,
    }))
    const batches = fit({
      page_load_id: 'x'.repeat(32),
      release: 'dev',
      page: '/login',
      events: big,
    })
    expect(batches.length).toBeGreaterThan(1)
    for (const b of batches) {
      expect(new TextEncoder().encode(JSON.stringify(b)).length).toBeLessThanOrEqual(
        MAX_BATCH_BYTES
      )
    }
    expect(
      batches.flatMap((b) => b.events.map((e) => (e.type === 'error' ? e.message : e.type)))
    ).toEqual(big.map((e) => e.message))
  })

  it('flushes on a timer once something is queued', () => {
    const { reporter, sent, timers } = setup()
    expect(timers).toHaveLength(0)
    reporter.capture(boom())
    reporter.capture(boom('Error: other'))
    expect(timers).toHaveLength(1)
    timers[0]()
    expect(sent).toHaveLength(1)
  })

  it('uses the real timer interval by default', () => {
    vi.useFakeTimers()
    try {
      const sent: string[] = []
      const reporter = new Reporter({
        release: 'dev',
        pathname: () => '/login',
        isSignedIn: () => false,
        send: (b) => sent.push(b),
      })
      reporter.capture(boom())
      vi.advanceTimersByTime(FLUSH_INTERVAL_MS - 1)
      expect(sent).toHaveLength(0)
      vi.advanceTimersByTime(1)
      expect(sent).toHaveLength(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('sends what was collected while signed out without cookies, even after signing in', () => {
    let path = '/register'
    const { reporter, sent, anonymous, signIn } = setup({ pathname: () => path })
    reporter.pageLoad()
    reporter.capture(boom('TypeError: before sign-in'))
    signIn(true) // e.g. signup completed on /set-password
    path = '/dashboard'
    reporter.pageLoad()
    reporter.capture(boom('TypeError: after sign-in'))
    reporter.flush()
    expect(sent.map((b) => b.page)).toEqual(['/register', '/dashboard'])
    expect(anonymous).toEqual([true, false])
    expect(sent[0].page_load_id).not.toBe(sent[1].page_load_id)
  })

  it('sends what was collected while signed in with cookies, also after signing out', () => {
    const { reporter, anonymous, signIn } = setup()
    signIn(true)
    reporter.capture(boom('TypeError: signed in'))
    signIn(false)
    reporter.capture(boom('TypeError: signed out'))
    reporter.flush()
    expect(anonymous).toEqual([false, true])
  })

  it('stays silent when sending, the session or the page throws', () => {
    const { reporter } = setup({
      send: () => {
        throw new Error('blocked')
      },
      isSignedIn: () => {
        throw new Error('no session')
      },
    })
    expect(() => reporter.capture(boom())).not.toThrow()
    expect(() => reporter.flush()).not.toThrow()
    const broken = setup({
      pathname: () => {
        throw new Error('no location')
      },
    })
    expect(() => broken.reporter.capture(boom())).not.toThrow()
  })

  it('falls back to dev for a release the server would reject', () => {
    const { reporter, sent } = setup({ release: 'not a release!' })
    reporter.capture(boom())
    reporter.flush()
    expect(sent[0].release).toBe('dev')
  })

  it('writes no cookie and no browser storage', () => {
    const local = vi.spyOn(Storage.prototype, 'setItem')
    const cookie = vi.spyOn(document, 'cookie', 'set')
    const { reporter } = setup()
    for (let i = 0; i < 5; i++) reporter.capture(boom(`Error: ${i}`))
    reporter.flush()
    expect(local).not.toHaveBeenCalled()
    expect(cookie).not.toHaveBeenCalled()
  })
})

describe('pageOf', () => {
  it('drops a trailing slash and keeps the root', () => {
    expect(pageOf('/dashboard/settings/')).toBe('/dashboard/settings')
    expect(pageOf('/')).toBe('/')
    expect(pageOf('/x'.repeat(200))).toHaveLength(200)
  })
})

describe('beaconSend', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('sends a JSON beacon to the endpoint', async () => {
    const beacon = vi.spyOn(navigator, 'sendBeacon').mockReturnValue(true)
    const fetchSpy = vi.spyOn(globalThis, 'fetch')
    beaconSend('{"a":1}')
    expect(beacon).toHaveBeenCalledOnce()
    const [url, blob] = beacon.mock.calls[0]
    expect(url).toBe(ENDPOINT)
    expect((blob as Blob).type).toBe('application/json')
    expect(await (blob as Blob).text()).toBe('{"a":1}')
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  it('sends an anonymous batch by fetch without cookies, never by beacon', () => {
    const beacon = vi.spyOn(navigator, 'sendBeacon').mockReturnValue(true)
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(null))
    beaconSend('{}', { anonymous: true })
    expect(beacon).not.toHaveBeenCalled()
    expect(fetchSpy).toHaveBeenCalledWith(
      ENDPOINT,
      expect.objectContaining({ method: 'POST', keepalive: true, credentials: 'omit' })
    )
  })

  it('falls back to a keepalive fetch when the beacon is refused', () => {
    vi.spyOn(navigator, 'sendBeacon').mockReturnValue(false)
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(null))
    beaconSend('{}')
    expect(fetchSpy).toHaveBeenCalledWith(
      ENDPOINT,
      expect.objectContaining({ method: 'POST', keepalive: true, credentials: 'same-origin' })
    )
  })

  it('falls back to fetch when the beacon throws, and swallows a failed fetch', async () => {
    vi.spyOn(navigator, 'sendBeacon').mockImplementation(() => {
      throw new Error('blocked by an extension')
    })
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new TypeError('offline'))
    expect(() => beaconSend('{}')).not.toThrow()
    expect(fetchSpy).toHaveBeenCalledOnce()
    await Promise.resolve()
  })
})

describe('Reporter navigations and island timings', () => {
  it('queues them for the page they belong to, never merged', () => {
    const { reporter, sent } = setup()
    const nav = {
      nav_type: 'soft' as const,
      view_transition: 'ran' as const,
      outcome: 'completed' as const,
      total_ms: 400,
      fetch_ms: 120,
    }
    reporter.navigation(nav, '/dashboard/settings/')
    reporter.navigation(nav, '/dashboard/settings')
    reporter.island(
      { name: 'SettingsPanel', stage: 'ready', nav_type: 'soft', ms: 310.6 },
      '/dashboard/settings'
    )
    reporter.flush()
    expect(sent).toHaveLength(1)
    expect(sent[0].page).toBe('/dashboard/settings')
    expect(sent[0].events).toEqual([
      { type: 'navigation', ...nav },
      { type: 'navigation', ...nav },
      { type: 'island', name: 'SettingsPanel', stage: 'ready', nav_type: 'soft', ms: 311 },
    ])
  })

  it('drops values the server would refuse, so the rest of the batch still goes', () => {
    const { reporter, sent, events } = setup()
    reporter.island({ name: 'HomePanel', stage: 'ready', nav_type: 'hard', ms: 600_001 }, '/login')
    reporter.island({ name: 'HomePanel', stage: 'ready', nav_type: 'hard', ms: NaN }, '/login')
    reporter.navigation(
      { nav_type: 'hard', view_transition: 'none', outcome: 'completed', total_ms: 700_000 },
      '/login'
    )
    reporter.navigation(
      { nav_type: 'soft', view_transition: 'ran', outcome: 'completed', fetch_ms: -1 },
      '/login'
    )
    reporter.capture(boom())
    reporter.flush()
    expect(sent.flatMap((b) => b.events).map((e) => e.type)).toEqual(['error'])
    expect(events()).toHaveLength(1)
  })
})
