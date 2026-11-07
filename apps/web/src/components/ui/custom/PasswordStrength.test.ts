import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-svelte'
import PasswordStrength from './PasswordStrength.svelte'

describe('PasswordStrength', () => {
  it('renders nothing until the user starts typing', async () => {
    const screen = render(PasswordStrength, { props: { password: '' } })
    await expect.element(screen.getByText(/characters/i)).not.toBeInTheDocument()
  })

  it('shows the length requirement using the configured minLength', async () => {
    const screen = render(PasswordStrength, { props: { password: 'a' } })
    await expect.element(screen.getByText('8 characters')).toBeInTheDocument()
  })

  it('prompts for letters or numbers when only special characters are entered', async () => {
    const screen = render(PasswordStrength, { props: { password: '!!!' } })
    await expect.element(screen.getByText(/add letters or numbers/i)).toBeInTheDocument()
  })

  it('shows how many more characters are needed when too short', async () => {
    const screen = render(PasswordStrength, { props: { password: 'abc' } })
    await expect.element(screen.getByText(/5 more characters needed/i)).toBeInTheDocument()
  })

  it('respects a custom minLength when counting remaining characters', async () => {
    const screen = render(PasswordStrength, { props: { password: 'abc', minLength: 4 } })
    await expect.element(screen.getByText(/1 more characters needed/i)).toBeInTheDocument()
  })

  it('suggests adding numbers once long enough but still letters-only', async () => {
    const screen = render(PasswordStrength, { props: { password: 'abcdefgh' } })
    await expect.element(screen.getByText(/add numbers for more security/i)).toBeInTheDocument()
  })

  it('reports a strong password when length, letters, and numbers are present', async () => {
    const screen = render(PasswordStrength, { props: { password: 'abcd1234' } })
    await expect.element(screen.getByText(/strong password/i)).toBeInTheDocument()
  })
})
