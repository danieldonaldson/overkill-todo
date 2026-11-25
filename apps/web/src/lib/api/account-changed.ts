// Told by the HTTP client, which can't depend on the session store (the store
// depends on the client).

let changed: () => void = () => {}
let ended: () => void = () => {}

/** A change was refused because another account is signed in (OT-015 design D6). */
export function onAccountChanged(handle: () => void): void {
  changed = handle
}

export function accountChanged(): void {
  changed()
}

/** The session is over and the client is about to leave the page. */
export function onSessionEnded(handle: () => void): void {
  ended = handle
}

export function sessionEnded(): void {
  ended()
}
