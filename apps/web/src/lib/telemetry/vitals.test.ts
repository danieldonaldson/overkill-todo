import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { onCLS, onINP } from 'web-vitals'

import { Reporter } from './reporter'
import {
  type EventEntry,
  InteractionTracker,
  type ShiftEntry,
  ShiftTracker,
  VisitTracker,
  installVitals,
  rate,
  selectorOf,
} from './vitals'

const click = (interactionId: number, duration: number, startTime = 100): EventEntry => ({
  interactionId,
  duration,
  startTime,
})

const shift = (value: number, startTime: number, hadRecentInput = false): ShiftEntry => ({
  value,
  startTime,
  hadRecentInput,
})

describe('rate', () => {
  it('uses the standard thresholds, good and poor boundaries included', () => {
    expect(rate('INP', 200)).toBe('good')
    expect(rate('INP', 201)).toBe('needs-improvement')
    expect(rate('INP', 500)).toBe('needs-improvement')
    expect(rate('INP', 501)).toBe('poor')
    expect(rate('CLS', 0.1)).toBe('good')
    expect(rate('CLS', 0.26)).toBe('poor')
    expect(rate('LCP', 4001)).toBe('poor')
    expect(rate('TTFB', 800)).toBe('good')
  })
})

describe('InteractionTracker', () => {
  it('takes the longest entry of each interaction and reports the worst', () => {
    const t = new InteractionTracker()
    // One click is pointerdown, pointerup and click: same interactionId.
    t.add(click(7, 48))
    t.add(click(7, 120))
    t.add(click(7, 64))
    t.add(click(14, 90))
    t.add({ duration: 300, startTime: 1 }) // no interactionId: a hover, not an interaction
    expect(t.value()?.latency).toBe(120)
    expect(t.estimatedCount).toBe(2)
  })

  it('uses the 98th percentile once there are more than 50 interactions', () => {
    const t = new InteractionTracker()
    for (let i = 1; i <= 120; i++) t.add(click(i * 7, i))
    // floor(120 / 50) = 2: skip the two longest (120, 119).
    expect(t.value(120)?.latency).toBe(118)
    expect(t.value(49)?.latency).toBe(120)
  })

  it('reports nothing without an interaction', () => {
    expect(new InteractionTracker().value()).toBeNull()
  })
})

describe('ShiftTracker', () => {
  it('reports the largest session window, ignoring shifts after input', () => {
    const t = new ShiftTracker()
    t.add(shift(0.05, 0))
    t.add(shift(0.05, 500)) // same window: 0.1
    t.add(shift(0.5, 600, true)) // just after input: ignored
    t.add(shift(0.08, 2000)) // over 1 s since the last: new window
    t.add(shift(0.04, 2900)) // 0.12
    expect(t.value().value).toBeCloseTo(0.12)
  })

  it('caps a window at 5 seconds', () => {
    const t = new ShiftTracker()
    for (let ms = 0; ms <= 6000; ms += 900) t.add(shift(0.01, ms))
    // 0, 900 … 4500 fit in the first window (6 shifts); 5400 starts a new one.
    expect(t.value().value).toBeCloseTo(0.06)
  })

  it('is zero and shift-free with nothing recorded', () => {
    const t = new ShiftTracker()
    expect(t.value().value).toBe(0)
    expect(t.hasShifts).toBe(false)
  })
})

describe('selectorOf', () => {
  it('names tags and classes up to the nearest ID, never text or attributes', () => {
    const root = document.createElement('section')
    root.id = 'settings'
    root.innerHTML =
      '<form class="b a"><button data-email="ann@example.com">Save ann@example.com</button></form>'
    document.body.append(root)
    const button = root.querySelector('button')
    expect(selectorOf(button)).toBe('#settings>form.a.b>button')
    root.remove()
  })

  it('stays within 100 characters', () => {
    const el = document.createElement('div')
    el.className = Array.from({ length: 40 }, (_, i) => `class-${i}`).join(' ')
    expect(selectorOf(el)!.length).toBeLessThanOrEqual(100)
  })
})

function visitHarness() {
  const reports: {
    name: string
    value: number
    rating: string
    page: string
    attribution?: string
  }[] = []
  let path = '/dashboard'
  let now = 0
  let count: number | null = null
  const visit = new VisitTracker({
    report: (v, page) => reports.push({ ...v, page }),
    pathname: () => path,
    now: () => now,
    interactionCount: () => count,
  })
  return {
    visit,
    reports,
    go: (p: string, at: number) => {
      path = p
      now = at
    },
    setCount: (c: number | null) => {
      count = c
    },
  }
}

describe('VisitTracker', () => {
  it('reports INP and CLS for the page the visit was on', () => {
    const h = visitHarness()
    h.visit.start({ fullLoad: true })
    h.visit.addEvent(click(7, 80, 50)) // buffered from before the script ran
    h.visit.addShift(shift(0.02, 10))
    h.visit.end()
    expect(h.reports).toEqual([
      { name: 'INP', value: 80, rating: 'good', page: '/dashboard', attribution: undefined },
      { name: 'CLS', value: 0.02, rating: 'good', page: '/dashboard', attribution: undefined },
    ])
  })

  it('ignores entries from before a swapped-in visit started', () => {
    const h = visitHarness()
    h.go('/dashboard/settings', 5000)
    h.visit.start()
    h.visit.addEvent(click(7, 600, 4900)) // the click that navigated here
    h.visit.addEvent(click(14, 240, 5100))
    h.visit.end()
    expect(h.reports.map((r) => [r.name, r.value, r.page])).toEqual([
      ['INP', 240, '/dashboard/settings'],
      ['CLS', 0, '/dashboard/settings'],
    ])
  })

  it('gives a visit with only quick interactions an INP of 8 ms', () => {
    const h = visitHarness()
    h.setCount(4)
    h.visit.start()
    h.setCount(6) // two interactions, both under the 40 ms entry threshold
    h.visit.end()
    expect(h.reports[0]).toMatchObject({ name: 'INP', value: 8, rating: 'good' })
  })

  it('reports no INP without an interaction, and CLS zero for a fresh visit', () => {
    const h = visitHarness()
    h.visit.start({ fullLoad: true })
    h.visit.end()
    expect(h.reports.map((r) => r.name)).toEqual(['CLS'])
  })

  it('reports CLS for a resumed visit only if something shifted', () => {
    const h = visitHarness()
    h.visit.start({ fresh: false })
    h.visit.end()
    expect(h.reports).toEqual([])
    h.visit.start({ fresh: false })
    h.visit.addShift(shift(0.3, 10))
    h.visit.end()
    expect(h.reports.map((r) => [r.name, r.rating])).toEqual([['CLS', 'poor']])
  })

  it('counts interactions from the visit start for the percentile', () => {
    const h = visitHarness()
    h.setCount(1000) // interactions before this visit
    h.visit.start({ fullLoad: true })
    for (let i = 1; i <= 10; i++) h.visit.addEvent(click(i * 7, i * 10))
    h.setCount(1010)
    h.visit.end()
    // 10 interactions in the visit: the worst, not p98 of 1010.
    expect(h.reports[0]).toMatchObject({ name: 'INP', value: 100 })
  })

  it('attributes only a poor INP', () => {
    const h = visitHarness()
    const button = document.createElement('button')
    h.visit.start({ fullLoad: true })
    h.visit.addEvent({ ...click(7, 700), target: button })
    h.visit.end()
    expect(h.reports[0]).toMatchObject({ rating: 'poor', attribution: 'button' })
    h.visit.start({ fullLoad: true })
    h.visit.addEvent({ ...click(7, 100), target: button })
    h.visit.end()
    expect(h.reports[2]).toMatchObject({ rating: 'good', attribution: undefined })
  })

  it('ignores a second end', () => {
    const h = visitHarness()
    h.visit.start({ fullLoad: true })
    h.visit.end()
    h.visit.end()
    expect(h.reports).toHaveLength(1)
  })
})

function lifecycle({ sampleRate = 1 } = {}) {
  const target = new EventTarget()
  let path = '/dashboard'
  let hidden = false
  const reports: [string, string][] = []
  const onHidden = vi.fn()
  const vitals = installVitals({
    report: (v, page) => reports.push([v.name, page]),
    sampleRate,
    onHidden,
    random: () => 0.5,
    target,
    isHidden: () => hidden,
    pathname: () => path,
    measure: false,
  })
  const fire = (type: string) => target.dispatchEvent(new Event(type))
  return {
    vitals,
    reports,
    onHidden,
    fire,
    navigate: (to: string) => {
      fire('astro:before-preparation')
      path = to
      fire('astro:after-swap')
      fire('astro:page-load')
    },
    setHidden: (h: boolean) => {
      hidden = h
      fire('visibilitychange')
    },
  }
}

describe('installVitals', () => {
  it('ends a visit as the next navigation starts and starts one on its page load', () => {
    const l = lifecycle()
    l.fire('astro:page-load') // the full load's own: its visit is already running
    l.navigate('/dashboard/settings')
    expect(l.reports).toEqual([['CLS', '/dashboard']])
    l.vitals!.visit.addEvent(click(7, 300, performance.now() + 1))
    l.navigate('/dashboard/admin')
    expect(l.reports.slice(1)).toEqual([
      ['INP', '/dashboard/settings'],
      ['CLS', '/dashboard/settings'],
    ])
  })

  it('ends a visit when the page is hidden and sends at once', () => {
    const l = lifecycle()
    l.setHidden(true)
    expect(l.reports).toEqual([['CLS', '/dashboard']])
    expect(l.onHidden).toHaveBeenCalledTimes(1)
    // Shown again: a resumed visit, which adds no zero row when hidden again.
    l.setHidden(false)
    l.setHidden(true)
    expect(l.reports).toHaveLength(1)
    expect(l.onHidden).toHaveBeenCalledTimes(1)
  })

  it('measures nothing when the full load is not sampled', () => {
    const l = lifecycle({ sampleRate: 0 })
    expect(l.vitals).toBeNull()
    l.navigate('/dashboard/settings')
    l.setHidden(true)
    expect(l.reports).toEqual([])
  })

  it('samples by comparing one random draw with the rate', () => {
    const common = { report: () => {}, measure: false, target: new EventTarget() }
    expect(installVitals({ ...common, sampleRate: 0.1, random: () => 0.05 })).not.toBeNull()
    expect(installVitals({ ...common, sampleRate: 0.1, random: () => 0.1 })).toBeNull()
  })
})

describe('Reporter.vital', () => {
  function reporter() {
    const sent: string[] = []
    const r = new Reporter({
      release: 'dev',
      pathname: () => '/dashboard/settings',
      isSignedIn: () => true,
      send: (body) => sent.push(body),
      setTimer: () => 0,
      clearTimer: () => {},
    })
    return { r, sent: () => sent.map((s) => JSON.parse(s)) }
  }

  it('queues the vital for the page it was measured on, rounded', () => {
    const { r, sent } = reporter()
    r.vital({ name: 'LCP', value: 2412.6, rating: 'good' }, '/dashboard/')
    r.vital({ name: 'CLS', value: 0.123456, rating: 'needs-improvement' }, '/dashboard/settings')
    r.flush()
    const batches = sent()
    expect(batches.map((b) => [b.page, b.events])).toEqual([
      ['/dashboard', [{ type: 'vital', name: 'LCP', value: 2413, rating: 'good' }]],
      [
        '/dashboard/settings',
        [{ type: 'vital', name: 'CLS', value: 0.1235, rating: 'needs-improvement' }],
      ],
    ])
  })

  it('keeps attribution only for a poor rating, scrubbed and capped', () => {
    const { r, sent } = reporter()
    r.vital({ name: 'INP', value: 700, rating: 'poor', attribution: 'x'.repeat(150) }, '/dashboard')
    r.vital(
      { name: 'INP', value: 300, rating: 'needs-improvement', attribution: 'button' },
      '/dashboard'
    )
    r.flush()
    const [poor, fine] = sent()[0].events
    expect(poor.attribution).toHaveLength(100)
    expect(fine.attribution).toBeUndefined()
  })

  it('drops values that are not finite and clamps the rest to the server range', () => {
    const { r, sent } = reporter()
    r.vital({ name: 'LCP', value: Number.NaN, rating: 'good' }, '/dashboard')
    r.vital({ name: 'LCP', value: -5, rating: 'good' }, '/dashboard')
    r.vital({ name: 'LCP', value: 10_000_000, rating: 'poor' }, '/dashboard')
    r.flush()
    expect(sent()[0].events.map((e: { value: number }) => e.value)).toEqual([0, 600_000])
  })
})

describe('against web-vitals in a real browser', () => {
  const nextFrame = () => new Promise((r) => requestAnimationFrame(() => setTimeout(r, 0)))

  it('gives the same INP and CLS for a full load with no swaps', async () => {
    let expectedInp: number | undefined
    let expectedCls: number | undefined
    onINP((m) => (expectedInp = m.value), { reportAllChanges: true })
    onCLS((m) => (expectedCls = m.value), { reportAllChanges: true })
    const reports: { name: string; value: number }[] = []
    const vitals = installVitals({
      report: (v) => reports.push(v),
      sampleRate: 1,
      random: () => 0,
      target: new EventTarget(),
      pathname: () => '/dashboard',
    })

    // Two layout shifts, before any input.
    const stage = document.createElement('div')
    const content = document.createElement('p')
    content.textContent = 'content that moves'
    stage.append(content)
    document.body.prepend(stage)
    await nextFrame()
    for (const height of [120, 60]) {
      const pusher = document.createElement('div')
      pusher.style.height = `${height}px`
      stage.prepend(pusher)
      await nextFrame()
    }
    await expect.poll(() => expectedCls ?? 0).toBeGreaterThan(0)

    // Two slow clicks: the handlers block the main thread.
    const button = document.createElement('button')
    button.textContent = 'slow'
    let block = 0
    button.addEventListener('click', () => {
      const until = performance.now() + block
      while (performance.now() < until) {
        // busy
      }
    })
    stage.append(button)
    for (const ms of [120, 260]) {
      block = ms
      await userEvent.click(button)
      await nextFrame()
    }
    await expect.poll(() => expectedInp ?? 0, { timeout: 5000 }).toBeGreaterThan(200)

    // Let both observers receive every entry, then end the visit.
    await new Promise((r) => setTimeout(r, 200))
    vitals!.visit.end()
    stage.remove()

    expect(reports.find((r) => r.name === 'INP')?.value).toBe(expectedInp)
    expect(reports.find((r) => r.name === 'CLS')?.value).toBeCloseTo(expectedCls!, 6)
  })
})
