import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import CountdownTimer from './CountdownTimer.svelte'

describe('CountdownTimer', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('formats the remaining time as M:SS', async () => {
    const screen = render(CountdownTimer, { props: { seconds: 90, autoStart: false } })

    await expect.element(screen.getByText('1:30')).toBeInTheDocument()
  })

  it('zero-pads the seconds component', async () => {
    const screen = render(CountdownTimer, { props: { seconds: 65, autoStart: false } })

    await expect.element(screen.getByText('1:05')).toBeInTheDocument()
  })

  it('renders a custom label before the time', async () => {
    const screen = render(CountdownTimer, {
      props: { seconds: 30, autoStart: false, label: 'Try again in' },
    })

    await expect.element(screen.getByText('Try again in', { exact: false })).toBeInTheDocument()
    await expect.element(screen.getByText('0:30')).toBeInTheDocument()
  })

  it('renders nothing once the countdown is complete', async () => {
    const screen = render(CountdownTimer, { props: { seconds: 0, autoStart: false } })

    await expect.element(screen.getByText('0:00')).not.toBeInTheDocument()
  })

  it('ticks down once per second while auto-started', async () => {
    const screen = render(CountdownTimer, { props: { seconds: 5, autoStart: true } })

    await expect.element(screen.getByText('0:05')).toBeInTheDocument()

    await vi.advanceTimersByTimeAsync(2000)

    await expect.element(screen.getByText('0:03')).toBeInTheDocument()
  })

  it('calls oncomplete and hides itself when it reaches zero', async () => {
    const oncomplete = vi.fn()
    const screen = render(CountdownTimer, { props: { seconds: 2, autoStart: true, oncomplete } })

    await vi.advanceTimersByTimeAsync(2000)

    expect(oncomplete).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByText('0:00')).not.toBeInTheDocument()
  })

  it('does not auto-start when autoStart is false', async () => {
    const oncomplete = vi.fn()
    const screen = render(CountdownTimer, { props: { seconds: 3, autoStart: false, oncomplete } })

    await vi.advanceTimersByTimeAsync(5000)

    expect(oncomplete).not.toHaveBeenCalled()
    await expect.element(screen.getByText('0:03')).toBeInTheDocument()
  })
})
