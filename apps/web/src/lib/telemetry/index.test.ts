import { describe, expect, it } from 'vitest'

import { fromErrorEvent, fromRejection } from './index'

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
