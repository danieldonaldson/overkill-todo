export type ValidationRule<T> = (value: T) => string | null

export function required(message = 'This field is required'): ValidationRule<string> {
  return (value) => (value?.trim() ? null : message)
}

export function email(message = 'Please enter a valid email address'): ValidationRule<string> {
  return (value) => (/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value) ? null : message)
}

export function minLength(min: number, message?: string): ValidationRule<string> {
  return (value) =>
    value.length >= min ? null : (message ?? `Must be at least ${min} characters`)
}

export function maxLength(max: number, message?: string): ValidationRule<string> {
  return (value) => (value.length <= max ? null : (message ?? `Must be at most ${max} characters`))
}

export function pattern(regex: RegExp, message: string): ValidationRule<string> {
  return (value) => (regex.test(value) ? null : message)
}

export function validate<T>(value: T, rules: ValidationRule<T>[]): string | null {
  for (const rule of rules) {
    const error = rule(value)
    if (error) return error
  }
  return null
}

export function validateAll<T extends Record<string, unknown>>(
  values: T,
  schema: { [K in keyof T]?: ValidationRule<T[K]>[] }
): Partial<Record<keyof T, string>> {
  const errors: Partial<Record<keyof T, string>> = {}

  for (const key in schema) {
    const rules = schema[key]
    if (!rules) continue

    const error = validate(values[key], rules as ValidationRule<unknown>[])
    if (error) errors[key] = error
  }

  return errors
}
