import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-svelte'
import Alert from './alert.svelte'

describe('Alert', () => {
  it('defaults to role="alert"', async () => {
    const screen = await render(Alert)

    await expect.element(screen.getByRole('alert')).toBeInTheDocument()
  })

  it('lets callers override the role, e.g. role="status" for success messages', async () => {
    const screen = await render(Alert, { props: { role: 'status', variant: 'success' } })

    await expect.element(screen.getByRole('status')).toBeInTheDocument()
    expect(screen.container.querySelector('[role="alert"]')).toBeNull()
  })
})
