// lists: Members open a list, add a todo and tick it, and sometimes start a new
// list (OT-013). run.sh clears the test Members' lists first, so every run
// starts from the same data and none reaches a limit.
import exec from 'k6/execution'
import { Client } from '../lib/client.js'
import { constantRun } from '../lib/run.js'
import { summaryFor } from '../lib/summary.js'
import { requireSeeded } from '../flows/auth.js'
import {
  addTodo,
  createList,
  listLists,
  markTodo,
  openList,
  resume,
  signInMembers,
} from '../flows/lists.js'

const MEMBERS = 40

const run = constantRun({
  scenario: 'lists',
  routes: [
    'GET /api/v1/lists',
    'POST /api/v1/lists',
    'GET /api/v1/lists/{id}',
    'POST /api/v1/lists/{id}/todos',
    'PATCH /api/v1/lists/{id}/todos/{todo_id}',
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
  if (n % 5 === 0) createList(client)
  listLists(client)
  openList(client, session.listId)
  const added = addTodo(client, session.listId)
  if (added.status === 200) markTodo(client, session.listId, added.json(), true)
}
