// What to do when the server refuses a change because another account is signed
// in (OT-015 design D6). Its own module so the HTTP client doesn't depend on the
// session store, which depends on the client.

let handler: () => void = () => {}

export function onAccountChanged(handle: () => void): void {
  handler = handle
}

export function accountChanged(): void {
  handler()
}
