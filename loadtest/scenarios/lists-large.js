// lists-large: the same list of 1,000 todos opened, ticked and reordered by several
// tabs at once (OT-013), so writes also queue on the list's lock. run.sh fills the lists.
import exec from 'k6/execution'
import { Client } from '../lib/client.js'
import { constantRun } from '../lib/run.js'
import { summaryFor } from '../lib/summary.js'
import { requireSeeded } from '../flows/auth.js'
import { listLists, markTodo, moveTodo, openList, resume, signInMembers } from '../flows/lists.js'

// Few Members, so several VUs share each list.
const MEMBERS = 5

const run = constantRun({
  scenario: 'lists-large',
  routes: [
    'GET /api/v1/lists/{id} (1000 todos)',
    'PATCH /api/v1/lists/{id}/todos/{todo_id}',
    'POST /api/v1/lists/{id}/todos/{todo_id}/move',
  ],
  rate: 10,
  warmup: '30s',
  duration: '3m',
  vus: 20,
  maxVUs: 40,
})

export const options = run.options
export const handleSummary = summaryFor('lists-large', run.plannedMs)

export function setup() {
  const problem = requireSeeded(new Client())
  if (problem) exec.test.abort(problem)
  const sessions = signInMembers(MEMBERS)
  if (!sessions) exec.test.abort('A test Member could not sign in.')
  return sessions.map((session) => {
    const [list] = listLists(resume(session)).json()
    if (!list || list.todo_count !== 1000) exec.test.abort('run.sh did not fill the lists.')
    return { ...session, listId: list.id }
  })
}

export default function (sessions) {
  const n = exec.scenario.iterationInTest
  const session = sessions[n % sessions.length]
  const client = resume(session)
  const opened = openList(client, session.listId, ' (1000 todos)')
  if (opened.status !== 200) return
  // A different todo per iteration, so tabs queue on the list but never get a
  // stale version.
  const todos = opened.json('todos').sort((a, b) => (a.id < b.id ? -1 : 1))
  const todo = todos[n % todos.length]
  markTodo(client, session.listId, todo, !todo.done)

  // Directly below the first open todo, where each move halves the gap, so about
  // one in 20 renumbers the list: the slowest list write. The moved todo is far
  // from the ones other tabs are ticking, so it is never done by then.
  const open = opened.json('todos').filter(({ done, id }) => !done && id !== todo.id)
  const far = todos[(n + 500) % todos.length]
  const moved = open.find(({ id }) => id === far.id) ?? open[open.length - 1]
  const [first, second] = open.filter(({ id }) => id !== moved.id)
  moveTodo(client, session.listId, moved, [first.id], [second.id])
}
