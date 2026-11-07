import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import CodeInput from './CodeInput.svelte'
import CodeInputHarness from './CodeInputHarness.test.svelte'

const getInput = (screen: ReturnType<typeof render>) =>
  screen.getByRole('textbox', { name: /verification code/i })

const pressKey = (input: HTMLInputElement, init: KeyboardEventInit) => {
  const event = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init })
  input.dispatchEvent(event)
  return event
}

describe('CodeInput', () => {
  it('formats a full 6-digit code with a space and reports completion', async () => {
    const oncomplete = vi.fn()
    const oninput = vi.fn()
    const screen = render(CodeInput, { props: { oncomplete, oninput } })

    await getInput(screen).fill('123456')

    await expect.element(getInput(screen)).toHaveValue('123 456')
    expect(oninput).toHaveBeenLastCalledWith('123456')
    expect(oncomplete).toHaveBeenCalledExactlyOnceWith('123456')
  })

  it('strips non-digit characters from input', async () => {
    const oncomplete = vi.fn()
    const oninput = vi.fn()
    const screen = render(CodeInput, { props: { oncomplete, oninput } })

    await getInput(screen).fill('a1b2c3')

    await expect.element(getInput(screen)).toHaveValue('123')
    expect(oninput).toHaveBeenLastCalledWith('123')
    expect(oncomplete).not.toHaveBeenCalled()
  })

  it('respects a custom length for completion', async () => {
    const oncomplete = vi.fn()
    const screen = render(CodeInput, { props: { length: 4, oncomplete } })

    await getInput(screen).fill('1234')

    expect(oncomplete).toHaveBeenCalledExactlyOnceWith('1234')
  })

  it('handles a pasted code, sanitizing and completing', async () => {
    const oncomplete = vi.fn()
    const oninput = vi.fn()
    const screen = render(CodeInput, { props: { oncomplete, oninput } })

    const input = getInput(screen).element() as HTMLInputElement
    const data = new DataTransfer()
    data.setData('text', '99-88-77')
    input.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true })
    )

    await expect.element(getInput(screen)).toHaveValue('998 877')
    expect(oninput).toHaveBeenLastCalledWith('998877')
    expect(oncomplete).toHaveBeenCalledExactlyOnceWith('998877')
  })

  it('renders an error message and marks the field invalid', async () => {
    const screen = render(CodeInput, { props: { error: 'Invalid code' } })

    await expect.element(screen.getByText('Invalid code')).toBeInTheDocument()
    await expect.element(getInput(screen)).toHaveAttribute('aria-invalid', 'true')
  })

  it('blocks non-digit key presses', async () => {
    const screen = render(CodeInput)
    const input = getInput(screen).element() as HTMLInputElement

    expect(pressKey(input, { key: 'a' }).defaultPrevented).toBe(true)
    expect(pressKey(input, { key: '/' }).defaultPrevented).toBe(true)
  })

  it('allows digit key presses', async () => {
    const screen = render(CodeInput)
    const input = getInput(screen).element() as HTMLInputElement

    expect(pressKey(input, { key: '5' }).defaultPrevented).toBe(false)
  })

  it('allows navigation/editing keys', async () => {
    const screen = render(CodeInput)
    const input = getInput(screen).element() as HTMLInputElement

    for (const key of ['Backspace', 'Delete', 'Tab', 'ArrowLeft', 'ArrowRight', 'Home', 'End']) {
      expect(pressKey(input, { key }).defaultPrevented).toBe(false)
    }
  })

  it('allows paste and select-all shortcuts', async () => {
    const screen = render(CodeInput)
    const input = getInput(screen).element() as HTMLInputElement

    expect(pressKey(input, { key: 'v', ctrlKey: true }).defaultPrevented).toBe(false)
    expect(pressKey(input, { key: 'v', metaKey: true }).defaultPrevented).toBe(false)
    expect(pressKey(input, { key: 'a', ctrlKey: true }).defaultPrevented).toBe(false)
  })

  it('focuses and clears via the exported imperative handle', async () => {
    const screen = render(CodeInputHarness)
    const input = getInput(screen)

    await input.fill('123456')
    await expect.element(input).toHaveValue('123 456')

    await screen.getByTestId('do-focus').click()
    expect(document.activeElement).toBe(input.element())

    await screen.getByTestId('do-clear').click()
    await expect.element(input).toHaveValue('')
  })
})
