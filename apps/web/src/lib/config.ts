// Public runtime config. In built images the values come from /config.js
// (`window.__PUBLIC_CONFIG`), which the dashboard container renders per-environment at
// startup from PUBLIC_* env (see docker-entrypoint.d), so
// one promoted image serves every environment. In the local dev server there is no rendered
// config.js — the placeholders survive — so we fall back to Astro's build-time import.meta.env
// (PUBLIC_* vars set on the dev container). Client-exposed vars must be prefixed PUBLIC_.

interface PublicConfig {
  TURNSTILE_SITE_KEY?: string
  UMAMI_WEBSITE_ID?: string
  RUM_SAMPLE_RATE?: string
}

const runtime: PublicConfig =
  (globalThis as { __PUBLIC_CONFIG?: PublicConfig }).__PUBLIC_CONFIG ?? {}

/**
 * The rendered runtime value if there is one, else the build-time env, else ''.
 * An unsubstituted "${VAR}" placeholder (dev, or an env the entrypoint left
 * unset) counts as unset so the build-time fallback wins. Exported for tests.
 */
export function resolvePublicValue(
  runtimeValue: string | undefined,
  buildValue: string | undefined,
): string {
  const rendered = runtimeValue && !runtimeValue.startsWith('${') ? runtimeValue : undefined
  return rendered ?? buildValue ?? ''
}

/**
 * Cloudflare Turnstile site key (public). Empty in local dev — the Turnstile
 * widget short-circuits on localhost and the backend skips validation in dev.
 */
export const TURNSTILE_SITE_KEY: string = resolvePublicValue(
  runtime.TURNSTILE_SITE_KEY,
  import.meta.env.PUBLIC_TURNSTILE_SITE_KEY,
)

/**
 * Umami website ID for web analytics (OT-005). Empty means analytics is off:
 * this is the feature flag. `public/scripts/analytics.js` reads the same value
 * from `window.__PUBLIC_CONFIG` to decide whether to load the tracker.
 */
export const UMAMI_WEBSITE_ID: string = resolvePublicValue(
  runtime.UMAMI_WEBSITE_ID,
  import.meta.env.PUBLIC_UMAMI_WEBSITE_ID,
)

/**
 * The share of full page loads that report Web Vitals (OT-006 Decision 6): a
 * number from 0 to 1. Unset, empty or anything else invalid means 1, so a typo
 * never silently turns measurement off. Exported for tests.
 */
export function parseSampleRate(value: string): number {
  if (value.trim() === '') return 1
  const rate = Number(value)
  return Number.isFinite(rate) && rate >= 0 && rate <= 1 ? rate : 1
}

/** `PUBLIC_RUM_SAMPLE_RATE`; errors and page loads are never sampled. */
export const RUM_SAMPLE_RATE: number = parseSampleRate(
  resolvePublicValue(runtime.RUM_SAMPLE_RATE, import.meta.env.PUBLIC_RUM_SAMPLE_RATE),
)

/**
 * Base path for all backend API calls. Relative so the browser resolves it
 * against the current origin (the dashboard is served behind the same proxy as
 * the API). Single source of truth — import this instead of re-declaring it.
 */
export const API_BASE = '/api/v1'
