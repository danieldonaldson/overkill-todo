import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import Turnstile from './Turnstile.svelte'

interface RenderOpts {
  sitekey: string
  callback: (token: string) => void
  'error-callback': () => void
  'expired-callback': () => void
  appearance: string
  theme: string
}

type TurnstileGlobal = {
  render: (el: HTMLElement, opts: RenderOpts) => string
  reset: (id: string) => void
  remove: (id: string) => void
}

function stubTurnstile(): { api: TurnstileGlobal; lastOpts: () => RenderOpts } {
  let captured: RenderOpts | undefined
  const api: TurnstileGlobal = {
    render: vi.fn((_el, opts) => {
      captured = opts
      return 'widget-1'
    }),
    reset: vi.fn(),
    remove: vi.fn(),
  }
  ;(window as unknown as { turnstile?: TurnstileGlobal }).turnstile = api
  return { api, lastOpts: () => captured as RenderOpts }
}

describe('Turnstile', () => {
  afterEach(() => {
    delete (window as unknown as { turnstile?: unknown }).turnstile
    delete (window as unknown as { onTurnstileLoad?: unknown }).onTurnstileLoad
    delete (window as unknown as { __turnstileQueue?: unknown }).__turnstileQueue
    document
      .querySelectorAll('script[src*="turnstile"]')
      .forEach((s) => s.remove())
    vi.restoreAllMocks()
  })

  it('auto-verifies with the dev token and renders no widget in dev mode', async () => {
    // The browser test harness serves from localhost, so isDev is true here.
    const onVerify = vi.fn()
    render(Turnstile, { props: { onVerify } })

    await vi.waitFor(() => expect(onVerify).toHaveBeenCalledWith('dev-mode-skip'))
    expect(document.querySelector('.turnstile-container')).toBeNull()
  })

  it('renders the widget in production mode when the script is already loaded', async () => {
    const { api } = stubTurnstile()
    const onVerify = vi.fn()
    render(Turnstile, { props: { onVerify, forceProduction: true } })

    await vi.waitFor(() => expect(api.render).toHaveBeenCalledTimes(1))
    expect(document.querySelector('.turnstile-container')).not.toBeNull()
  })

  it('stays invisible unless Cloudflare wants to challenge the visitor', async () => {
    const { api, lastOpts } = stubTurnstile()
    const onVerify = vi.fn()
    render(Turnstile, { props: { onVerify, forceProduction: true } })

    await vi.waitFor(() => expect(api.render).toHaveBeenCalled())
    expect(lastOpts().appearance).toBe('interaction-only')
  })

  it('forwards the success token and clears it on error/expiry', async () => {
    const { api, lastOpts } = stubTurnstile()
    const onVerify = vi.fn()
    render(Turnstile, { props: { onVerify, forceProduction: true } })

    await vi.waitFor(() => expect(api.render).toHaveBeenCalled())
    const opts = lastOpts()

    opts.callback('real-token')
    expect(onVerify).toHaveBeenCalledWith('real-token')

    opts['error-callback']()
    opts['expired-callback']()
    expect(onVerify).toHaveBeenCalledWith('')
    expect(onVerify).toHaveBeenCalledTimes(3)
  })

  it('injects the Cloudflare script and drains the render queue on load', async () => {
    const onVerify = vi.fn()
    render(Turnstile, { props: { onVerify, forceProduction: true } })

    await vi.waitFor(() =>
      expect(document.querySelector('script[src*="turnstile"]')).not.toBeNull()
    )
    const onload = (window as unknown as { onTurnstileLoad?: () => void }).onTurnstileLoad
    expect(typeof onload).toBe('function')

    // Simulate the Cloudflare script finishing: register the global, then drain.
    const { api } = stubTurnstile()
    onload?.()
    expect(api.render).toHaveBeenCalledTimes(1)
  })
})
