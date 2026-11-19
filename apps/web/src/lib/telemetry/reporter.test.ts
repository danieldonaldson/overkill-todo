import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  type Batch,
  ENDPOINT,
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
  let signedIn = false
  let ids = 0
  const timers: Array<() => void> = []
  const reporter = new Reporter({
    release: 'abc123',
    pathname: () => '/login',
    isSignedIn: () => signedIn,
    send: (body) => sent.push(JSON.parse(body) as Batch),
    newPageLoadId: () => String(++ids).padStart(32, '0'),
    setTimer: (fn) => timers.push(fn),
    clearTimer: () => {},
    ...overrides,
  })
  return {
    reporter,
    sent,
    timers,
    signIn: (value: boolean) => {
      signedIn = value
    },
    events: () => sent.flatMap((b) => b.events),
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
    expect(batches.flatMap((b) => b.events.map((e) => e.message))).toEqual(
      big.map((e) => e.message)
    )
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
