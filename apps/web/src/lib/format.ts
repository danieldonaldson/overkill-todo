// Small display formatters shared across dashboard panels.

/** Locale date (no time); echoes the input unchanged if it isn't a valid date. */
export function formatDate(iso: string): string {
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleDateString()
}

/** Locale date + time; echoes the input unchanged if it isn't a valid date. */
export function formatDateTime(iso: string): string {
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}
