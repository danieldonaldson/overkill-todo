import { describe, expect, it, vi } from 'vitest'

import { fromApiFailure, fromErrorEvent, fromRejection, watchPageLoads } from './index'

describe('error capture', () => {
  it('reads an uncaught error', () => {
    const error = new TypeError('x is undefined')
    const captured = fromErrorEvent(
      new ErrorEvent('error', {
        error,
        message: 'Uncaught TypeError: x is undefined',
        filename: location.href,
      })
    )
    expect(captured).toMatchObject({
      kind: 'error',
      message: 'TypeError: x is undefined',
      source: location.href,
    })
    expect(captured.stack).toBe(error.stack)
  })

  it('marks errors from another origin as external', () => {
    expect(fromErrorEvent(new ErrorEvent('error', { message: 'Script error.' }))).toMatchObject({
      message: 'Script error.',
      source: 'external',
    })
    expect(
      fromErrorEvent(
        new ErrorEvent('error', { error: new Error('x'), filename: 'https://ext.example/a.js' })
      ).source
    ).toBe('external')
  })

  it('reads a rejection with an error or any other value', () => {
    const reason = new Error('nope')
    expect(
      fromRejection(
        new PromiseRejectionEvent('unhandledrejection', { promise: Promise.resolve(), reason })
      )
    ).toMatchObject({ kind: 'unhandledrejection', message: 'Error: nope', stack: reason.stack })
    expect(
      fromRejection(
        new PromiseRejectionEvent('unhandledrejection', { promise: Promise.resolve(), reason: 42 })
      ).message
    ).toBe('Non-error value: 42')
  })
})

describe('watchPageLoads', () => {
  it('counts the full load once and each in-site navigation once', () => {
    const target = new EventTarget()
    const count = vi.fn()
    watchPageLoads(target, count)
    expect(count).toHaveBeenCalledTimes(1)

    // Astro's own first page-load event for the full load: already counted.
    target.dispatchEvent(new Event('astro:page-load'))
    expect(count).toHaveBeenCalledTimes(1)

    for (let i = 0; i < 2; i++) {
      target.dispatchEvent(new Event('astro:after-swap'))
      expect(count).toHaveBeenCalledTimes(1 + i)
      target.dispatchEvent(new Event('astro:page-load'))
      expect(count).toHaveBeenCalledTimes(2 + i)
    }
  })
})

describe('fromApiFailure', () => {
  it('describes a 5xx with its trace ID, and a network failure without one', () => {
    const trace = 'abcdef0123456789abcdef0123456789'
    expect(
      fromApiFailure({ method: 'PATCH', route: '/users/me', status: 500, traceId: trace })
    ).toEqual({
      kind: 'api',
      message: 'PATCH /users/me failed: status 500',
      apiRoute: '/users/me',
      status: 500,
      networkFailed: undefined,
      traceId: trace,
    })
    expect(
      fromApiFailure({ method: 'POST', route: '/auth/register', networkFailed: true })
    ).toMatchObject({
      message: 'POST /auth/register failed: network error',
      networkFailed: true,
      status: undefined,
      traceId: undefined,
    })
  })
})
