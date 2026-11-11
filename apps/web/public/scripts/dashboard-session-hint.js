// Session hint check for dashboard pages, run render-blocking in <head> so a
// browser that has never signed in (or has signed out) goes to /login before
// any dashboard content is shown. A local read only: an expired-but-present
// hint still counts, because the refresh cookie may still work and the
// dashboard's background account check settles it.
//
// The key must match TOKEN_EXPIRY_KEY in src/lib/auth/token.ts (this script is
// served as-is and cannot import it).
//
// If localStorage itself throws (disabled storage), do nothing: the dashboard's
// account check still runs and the API still refuses unauthenticated calls.
;(function () {
  var TOKEN_EXPIRY_KEY = 'jwt_expiry'
  try {
    if (localStorage.getItem(TOKEN_EXPIRY_KEY) === null) location.replace('/login')
  } catch {
    // Storage unavailable: leave the page to the in-app check.
  }
})()
