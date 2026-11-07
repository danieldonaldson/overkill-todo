// Public runtime config. In built images the values come from /config.js
// (`window.__PUBLIC_CONFIG`), which the dashboard container renders per-environment at
// startup from PUBLIC_* env (see docker-entrypoint.d), so
// one promoted image serves every environment. In the local dev server there is no rendered
// config.js — the placeholders survive — so we fall back to Astro's build-time import.meta.env
// (PUBLIC_* vars set on the dev container). Client-exposed vars must be prefixed PUBLIC_.

interface PublicConfig {
  TURNSTILE_SITE_KEY?: string
}

const runtime: PublicConfig =
  (globalThis as { __PUBLIC_CONFIG?: PublicConfig }).__PUBLIC_CONFIG ?? {}

// An unsubstituted "${VAR}" placeholder (dev, or an env the entrypoint left unset) counts as
// unset so the import.meta.env fallback wins.
function runtimeValue(v: string | undefined): string | undefined {
  return v && !v.startsWith('${') ? v : undefined
}

/**
 * Cloudflare Turnstile site key (public). Empty in local dev — the Turnstile
 * widget short-circuits on localhost and the backend skips validation in dev.
 */
export const TURNSTILE_SITE_KEY: string =
  runtimeValue(runtime.TURNSTILE_SITE_KEY) ?? import.meta.env.PUBLIC_TURNSTILE_SITE_KEY ?? ''

/**
 * Base path for all backend API calls. Relative so the browser resolves it
 * against the current origin (the dashboard is served behind the same proxy as
 * the API). Single source of truth — import this instead of re-declaring it.
 */
export const API_BASE = '/api/v1'
