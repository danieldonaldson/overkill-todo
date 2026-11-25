// lists: Members open a list, add a todo, tick or rename-and-delete it, and
// sometimes create, rename and delete a list (OT-013). run.sh clears the test
// Members' lists first, so every run starts from the same data and none reaches
// a limit.
import exec from 'k6/execution'
import { Client } from '../lib/client.js'
import { constantRun } from '../lib/run.js'
import { summaryFor } from '../lib/summary.js'
import { requireSeeded } from '../flows/auth.js'
import {
  addTodo,
  createList,
  deleteList,
  deleteTodo,
  listLists,
  markTodo,
  openList,
  renameList,
  renameTodo,
  resume,
  signInMembers,
} from '../flows/lists.js'

const MEMBERS = 40

const run = constantRun({
  scenario: 'lists',
  routes: [
    'GET /api/v1/lists',
    'POST /api/v1/lists',
    'PATCH /api/v1/lists/{id}',
    'DELETE /api/v1/lists/{id}',
    'GET /api/v1/lists/{id}',
    'POST /api/v1/lists/{id}/todos',
    'PATCH /api/v1/lists/{id}/todos/{todo_id}',
    'DELETE /api/v1/lists/{id}/todos/{todo_id}',
  ],
  rate: 10,
  warmup: '30s',
  duration: '3m',
  vus: 20,
  maxVUs: 40,
})

export const options = run.options
export const handleSummary = summaryFor('lists', run.plannedMs)

export function setup() {
  const problem = requireSeeded(new Client())
  if (problem) exec.test.abort(problem)
  const sessions = signInMembers(MEMBERS)
  if (!sessions) exec.test.abort('A test Member could not sign in.')
  return sessions.map((session) => ({
    ...session,
    listId: createList(resume(session)).json('id'),
  }))
}

export default function (sessions) {
  const n = exec.scenario.iterationInTest
  const session = sessions[n % sessions.length]
  const client = resume(session)

  // A list created for this iteration only, so its version and count are ours.
  if (n % 5 === 0) {
    const created = createList(client)
    if (created.status === 200) {
      const list = created.json()
      const renamed = renameList(client, list, 'Renamed load list')
      if (renamed.status === 200) deleteList(client, renamed.json(), 0)
    }
  }

  listLists(client)
  openList(client, session.listId)
  const added = addTodo(client, session.listId)
  if (added.status === 200) {
    const todo = added.json()
    if (n % 2 === 0) {
      const renamed = renameTodo(client, session.listId, todo, 'Renamed load todo')
      if (renamed.status === 200) deleteTodo(client, session.listId, renamed.json())
    } else {
      markTodo(client, session.listId, todo, true)
    }
  }
}
