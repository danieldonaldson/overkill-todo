// Web analytics loader (OT-005, design D5). Loads Umami's tracker from our own
// origin (/stats/, routed by the edge) only when analytics is configured and
// the browser hasn't opted out. Kept as an external same-origin script so the
// CSP needs no new hash or origin.
//
// Off (does nothing, makes no request) when:
// - UMAMI_WEBSITE_ID is unset, empty or an unrendered ${...} placeholder
//   (public/config.js as served by the dev server), or
// - the browser sends Do Not Track or Global Privacy Control. Umami's own
//   data-do-not-track covers DNT only, hence the GPC check here.
//
// The tracker strips query strings and fragments from both the page URL and
// the referrer, follows history.pushState (view-transition moves count), and
// sets no cookie and writes no storage.
;(function () {
  if (window.__analyticsLoaded) return
  window.__analyticsLoaded = true

  var config = window.__PUBLIC_CONFIG || {}
  var id = config.UMAMI_WEBSITE_ID
  if (typeof id !== 'string' || id === '' || id.indexOf('${') === 0) return

  var nav = window.navigator
  if (nav.doNotTrack === '1' || window.doNotTrack === '1' || nav.globalPrivacyControl === true) {
    return
  }

  // Every list address is one page (OT-013 design D9): the tracker's payload
  // is rewritten before it is sent, and a list page's title is pinned. The
  // Anything that can't be rewritten is not sent. The
  // tracker resolves `data-before-send` on `window` before the first send.
  window.__otAnalyticsBeforeSend = function (type, payload) {
    try {
      if (!payload || typeof payload !== 'object') return payload
      var out = Object.assign({}, payload)
      var rewrite = function (value) {
        if (typeof value !== 'string' || value === '') return value
        try {
          var absolute = /^[a-z]+:\/\//i.test(value)
          var url = new URL(value, window.location.origin)
          if (/^\/dashboard\/lists\/.+/.test(url.pathname)) {
            url.pathname = '/dashboard/lists/[id]'
            url.search = ''
            url.hash = ''
            return absolute ? url.href : url.pathname
          }
        } catch {
          return ''
        }
        return value
      }
      out.url = rewrite(out.url)
      if (out.referrer) out.referrer = rewrite(out.referrer)
      if (typeof out.url === 'string' && /\/dashboard\/lists\/\[id\]$/.test(out.url)) {
        out.title = 'Lists'
      }
      return out
    } catch {
      return null
    }
  }

  var s = document.createElement('script')
  s.defer = true
  s.src = '/stats/script.js'
  s.setAttribute('data-website-id', id)
  s.setAttribute('data-host-url', window.location.origin + '/stats')
  s.setAttribute('data-exclude-search', 'true')
  s.setAttribute('data-exclude-hash', 'true')
  s.setAttribute('data-do-not-track', 'true')
  s.setAttribute('data-before-send', '__otAnalyticsBeforeSend')
  document.head.appendChild(s)
})()
