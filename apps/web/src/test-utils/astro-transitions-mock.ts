export function navigate(href: string | URL) {
  window.history.pushState({}, '', href)
}
