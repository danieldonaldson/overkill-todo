// Public runtime config for the app. Shipped with ${VAR} placeholders; the container
// entrypoint (docker-entrypoint.d/40-render-public-config.sh) renders these
// from the pod's PUBLIC_* env into /tmp/config.js at startup, which nginx serves as /config.js
// (the root filesystem is read-only, so it can't be rewritten in place). The local dev server
// serves this file verbatim — the placeholders survive (they start with "${" so config.ts's
// runtimeValue() rejects them) and config.ts falls back to import.meta.env. Loaded
// render-blocking in <head> so window.__PUBLIC_CONFIG is set before the app hydrates.
//
// NOTE: this is the DEV/source form. In built images the entrypoint does NOT substitute into
// these literals; it renders an equivalent `window.__PUBLIC_CONFIG = JSON.parse('…')` with the
// operator-set values JSON-encoded, so a value containing a quote/backslash/newline can't break
// the config. See src/lib/config.ts and docker-entrypoint.d.
window.__PUBLIC_CONFIG = {
  TURNSTILE_SITE_KEY: '${PUBLIC_TURNSTILE_SITE_KEY}',
}
