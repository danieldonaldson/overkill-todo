// Todo list flows (OT-013): what a Member does on a list page.
import { me, signIn } from './auth.js'
import { Client } from '../lib/client.js'
import { testEmail } from '../lib/config.js'

/** RFC 4122 v4-shaped key; only uniqueness matters here. */
function newKey() {
  return 'xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx'.replace(/x/g, () =>
    Math.floor(Math.random() * 16).toString(16)
  )
}

/**
 * Sign in Members 0..count-1, for `setup()`: hashing cost stays out of the
 * measured phase. Returns plain data for VUs to resume with `resume`.
 */
export function signInMembers(count) {
  const sessions = []
  for (let i = 0; i < count; i++) {
    const client = new Client()
    if (signIn(client, testEmail(i)).status !== 200) return null
    sessions.push({
      cookies: { ...client.cookies },
      csrfToken: client.csrfToken,
      accountId: me(client).json('id'),
    })
    client.reset()
  }
  return sessions
}

export function resume(session) {
  return Object.assign(new Client(), {
    cookies: { ...session.cookies },
    csrfToken: session.csrfToken,
    accountId: session.accountId,
  })
}

export function listLists(client) {
  return client.request('GET', '/lists')
}

export function createList(client, name = 'Load list') {
  return client.request('POST', '/lists', { body: { key: newKey(), name } })
}

/** `label` sets a list of a known size apart in the results. */
export function openList(client, id, label = '') {
  return client.request('GET', `/lists/{id}${label}`, { path: `/lists/${id}` })
}

export function addTodo(client, listId, title = 'A todo of ordinary length') {
  return client.request('POST', '/lists/{id}/todos', {
    path: `/lists/${listId}/todos`,
    body: { key: newKey(), title },
  })
}

export function markTodo(client, listId, todo, done) {
  return client.request('PATCH', '/lists/{id}/todos/{todo_id}', {
    path: `/lists/${listId}/todos/${todo.id}`,
    body: { version: todo.version, done },
  })
}
