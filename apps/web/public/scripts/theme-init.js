// Anti-flash: apply the saved theme before paint. mode-watcher reads the same
// class. Kept as an external same-origin script so the CSP can drop
// script-src 'unsafe-inline'.
;(function () {
  var saved = localStorage.getItem('mode-watcher-mode')
  if (saved === 'dark') document.documentElement.classList.add('dark')
  else if (saved === 'light') document.documentElement.classList.add('light')
})()
