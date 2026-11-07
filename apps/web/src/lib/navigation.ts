import { navigate as astroNavigate } from 'astro:transitions/client'

export function navigate(url: string): void {
  astroNavigate(url)
}

export function navigateReplace(url: string): void {
  window.location.replace(url)
}

/**
 * Performs a full-page redirect via window.location.href.
 * Used for hard navigations such as bouncing to /login on session expiry.
 */
export function redirect(url: string): void {
  window.location.href = url
}

export function goBack(): void {
  window.history.back()
}
