// -*- mode: Bluespec; -*-
//
// The lists client queue (OT-013 design D7, D13): two browser tabs editing the
// same todos through apps/web/src/lib/lists/engine.ts, against a server that
// follows D4. Prototype (task 4.4): no additions, moves, resets or account
// switches, and not yet run by scripts/models.sh (task 8.2).
//
//   Member   act (mark, rename or delete), startDraft, submitDraft
//   queue    send (the head, one at a time), answer, timeOut (retried once with
//            the same version, then dropped)
//   server   apply, at any time after send: also after a timeout, a retry or a
//            refetch. A request that is never applied was lost on the way.
//   load     loadRead (the server's state), loadAnswer (dropped if a save was
//            sent meanwhile; one read during a save would always be dropped)
//
// Bounds: see the modules at the end. Titles are small integers.
module list_queue {
  const TODOS: Set[int]
  const MAX_QUEUE: int
  const MAX_REQUESTS: int
  // Broken variants: what a refetch must never do.
  const REFETCH_ADVANCES_QUEUE: bool
  const REFETCH_ADVANCES_DRAFT: bool
  // Advance later edits after any success at `version + 1`, also one that asked
  // for what `version` already held (the bug task 4.4 found).
  const REBASE_ANY_SUCCESS: bool

  type Todo = { exists: bool, title: int, done: bool, version: int }
  // `seen`: ghost, the newest confirmed version on screen when the Member acted.
  // `id`: ghost, names a submitted text.
  type Op = { kind: str, todo: int, version: int, title: int, done: bool, seen: int, id: int }
  type Answer = { kind: str, todo: Todo }
  type Request = { client: int, op: Op, applied: bool, answer: Answer }
  type Draft = { open: bool, todo: int, version: int, before: int, seen: int }
  type Load = { open: bool, snap: int -> Todo }
  // `request`: the one the queue waits for, or -1.
  type Client = { conf: int -> Todo, q: List[Op], request: int, attempt: int,
                  draft: Draft, load: Load }

  pure val CLIENTS = Set(1, 2)
  pure val TITLES = Set(0, 1, 2)
  pure val NO_DRAFT: Draft = { open: false, todo: 0, version: 0, before: 0, seen: 0 }
  pure val FIRST: Todo = { exists: true, title: 0, done: false, version: 1 }

  var server: int -> Todo
  var clients: int -> Client
  var requests: int -> Request
  var nextRequest: int
  var nextId: int

  // Ghosts.
  // What each version of a todo held: (todo, version) -> contents.
  var history: Set[{ todo: int, version: int, title: int, done: bool }]
  // Versions that are a client's own: it made them, or asked for exactly that
  // change and found it made.
  var own: Set[{ client: int, todo: int, version: int }]
  // Every rename text submitted, and what became of it.
  var submitted: Set[int]
  var saved: Set[int]
  var offered: Set[int]
  // An applied edit changed a version its tab had neither seen nor made.
  var overwrote: bool

  // Every version after the one seen, up to the one sent, is the client's own.
  def mayEdit(c: int, t: int, seen: int, version: int): bool =
    history.forall(h =>
      (h.todo == t and h.version > seen and h.version <= version) implies
        own.contains({ client: c, todo: t, version: h.version }))

  // ── The engine's view (applyQueueToTodos) ───────────────────────────────

  pure def view(client: Client, t: int): Todo =
    client.q.foldl(client.conf.get(t), (todo, op) =>
      if (op.todo != t or not(todo.exists)) todo
      else if (op.kind == "delete") todo.with("exists", false)
      else if (op.kind == "rename") todo.with("title", op.title).with("version", op.version)
      else todo.with("done", op.done).with("version", op.version))

  // ── The Member ──────────────────────────────────────────────────────────

  action enqueue(c: int, op: Op): bool = {
    val me = clients.get(c)
    all {
      me.q.length() < MAX_QUEUE,
      clients' = clients.set(c, me.with("q", me.q.append(op))),
      server' = server, requests' = requests, nextRequest' = nextRequest,
      history' = history, own' = own, saved' = saved, offered' = offered, overwrote' = overwrote,
    }
  }

  action act(c: int, t: int, kind: str): bool = {
    val shown = view(clients.get(c), t)
    all {
      shown.exists,
      enqueue(c, { kind: kind, todo: t, version: shown.version, title: 0,
                   done: not(shown.done), seen: clients.get(c).conf.get(t).version, id: 0 }),
      nextId' = nextId, submitted' = submitted,
    }
  }

  action startDraft(c: int, t: int): bool = {
    val me = clients.get(c)
    val shown = view(me, t)
    all {
      shown.exists,
      not(me.draft.open),
      clients' = clients.set(c, me.with("draft",
        { open: true, todo: t, version: shown.version, before: shown.title,
          seen: me.conf.get(t).version })),
      server' = server, requests' = requests, nextRequest' = nextRequest, nextId' = nextId,
      history' = history, own' = own, submitted' = submitted, saved' = saved,
      offered' = offered, overwrote' = overwrote,
    }
  }

  action submitDraft(c: int, title: int): bool = {
    val me = clients.get(c)
    val d = me.draft
    all {
      d.open,
      title != d.before,
      me.q.length() < MAX_QUEUE,
      clients' = clients.set(c, me.with("draft", NO_DRAFT).with("q", me.q.append(
        { kind: "rename", todo: d.todo, version: d.version, title: title, done: false,
          seen: d.seen, id: nextId }))),
      nextId' = nextId + 1,
      submitted' = submitted.union(Set(nextId)),
      server' = server, requests' = requests, nextRequest' = nextRequest,
      history' = history, own' = own, saved' = saved, offered' = offered, overwrote' = overwrote,
    }
  }

  // ── Requests ────────────────────────────────────────────────────────────

  action send(c: int): bool = {
    val me = clients.get(c)
    all {
      me.request == -1,
      me.q.length() > 0,
      nextRequest < MAX_REQUESTS,
      requests' = requests.set(nextRequest, { client: c, op: me.q.head(), applied: false,
        answer: { kind: "none", todo: FIRST } }),
      nextRequest' = nextRequest + 1,
      clients' = clients.set(c, me.with("request", nextRequest).with("attempt", 1)
        .with("load", me.load.with("open", false))),
      server' = server, nextId' = nextId, history' = history, own' = own,
      submitted' = submitted, saved' = saved, offered' = offered, overwrote' = overwrote,
    }
  }

  // The server's rules (D4): 404, the state already stored, stale, or applied.
  action apply(r: int): bool = {
    val request = requests.get(r)
    val op = request.op
    val c = request.client
    val now = server.get(op.todo)
    val stored = now.exists and or {
      op.kind == "rename" and now.title == op.title,
      op.kind == "mark" and now.done == op.done,
    }
    val applies = now.exists and not(stored) and now.version == op.version
    val after =
      if (not(applies)) now
      else if (op.kind == "delete") now.with("exists", false)
      else if (op.kind == "rename") now.with("title", op.title).with("version", now.version + 1)
      else now.with("done", op.done).with("version", now.version + 1)
    val kind =
      if (not(now.exists)) "gone"
      else if (stored or applies) "ok"
      else "stale"
    // The version after the one sent holds this change, and the one sent didn't.
    val askedFor = stored and now.version == op.version + 1 and
      history.exists(h => h.todo == op.todo and h.version == op.version and
        if (op.kind == "rename") h.title != op.title else h.done != op.done)
    all {
      r < nextRequest,
      not(request.applied),
      server' = server.set(op.todo, after),
      requests' = requests.set(r, request.with("applied", true)
        .with("answer", { kind: kind, todo: after })),
      history' = history.union(if (applies and op.kind != "delete")
        Set({ todo: op.todo, version: after.version, title: after.title, done: after.done })
        else Set()),
      own' = own.union(if ((applies and op.kind != "delete") or askedFor)
        Set({ client: c, todo: op.todo, version: after.version }) else Set()),
      overwrote' = (overwrote or (applies and not(mayEdit(c, op.todo, op.seen, op.version)))),
      clients' = clients, nextRequest' = nextRequest, nextId' = nextId,
      submitted' = submitted, saved' = saved, offered' = offered,
    }
  }

  // Drop the head and every later edit of its todo, offering their texts again.
  def dropped(me: Client, t: int): Set[int] =
    me.q.indices().filter(i => me.q[i].todo == t and me.q[i].kind == "rename").map(i => me.q[i].id)

  action answer(c: int): bool = {
    val me = clients.get(c)
    val op = me.q.head()
    val a = requests.get(me.request).answer
    val known = me.conf.get(op.todo)
    val rebases = a.kind == "ok" and op.kind != "delete" and a.todo.version == op.version + 1 and
      or {
        REBASE_ANY_SUCCESS,
        known.version == op.version and
          if (op.kind == "rename") known.title != op.title else known.done != op.done,
      }
    val rest = me.q.tail()
    val queue =
      if (a.kind == "ok")
        rest.foldl(List(), (l, o) => l.append(
          if (rebases and o.todo == op.todo and o.version == op.version)
            o.with("version", a.todo.version) else o))
      else rest.select(o => o.todo != op.todo)
    val draft =
      if (rebases and me.draft.open and me.draft.todo == op.todo and me.draft.version == op.version)
        me.draft.with("version", a.todo.version)
      else me.draft
    val shown =
      if (a.kind == "gone" or op.kind == "delete" and a.kind == "ok") known.with("exists", false)
      else a.todo
    all {
      me.request != -1,
      requests.get(me.request).applied,
      clients' = clients.set(c, me.with("q", queue).with("request", -1).with("draft", draft)
        .with("conf", me.conf.set(op.todo, shown))),
      saved' = if (a.kind == "ok" and op.kind == "rename") saved.union(Set(op.id)) else saved,
      offered' = if (a.kind == "ok") offered else offered.union(dropped(me, op.todo)),
      server' = server, requests' = requests, nextRequest' = nextRequest, nextId' = nextId,
      history' = history, own' = own, submitted' = submitted, overwrote' = overwrote,
    }
  }

  // No answer: repeat once with the same version, then drop. The earlier
  // request may still be applied later.
  action timeOut(c: int): bool = {
    val me = clients.get(c)
    all {
      me.request != -1,
      if (me.attempt == 1) all {
        nextRequest < MAX_REQUESTS,
        requests' = requests.set(nextRequest, { client: c, op: me.q.head(), applied: false,
          answer: { kind: "none", todo: FIRST } }),
        nextRequest' = nextRequest + 1,
        clients' = clients.set(c, me.with("request", nextRequest).with("attempt", 2)),
        offered' = offered,
      } else all {
        requests' = requests,
        nextRequest' = nextRequest,
        clients' = clients.set(c, me.with("request", -1)
          .with("q", me.q.select(o => o.todo != me.q.head().todo))),
        offered' = offered.union(dropped(me, me.q.head().todo)),
      },
      server' = server, nextId' = nextId, history' = history, own' = own,
      submitted' = submitted, saved' = saved, overwrote' = overwrote,
    }
  }

  // ── Loads ───────────────────────────────────────────────────────────────

  action loadRead(c: int): bool = {
    val me = clients.get(c)
    all {
      me.request == -1,
      not(me.load.open),
      clients' = clients.set(c, me.with("load", { open: true, snap: server })),
      server' = server, requests' = requests, nextRequest' = nextRequest, nextId' = nextId,
      history' = history, own' = own, submitted' = submitted, saved' = saved,
      offered' = offered, overwrote' = overwrote,
    }
  }

  action loadAnswer(c: int): bool = {
    val me = clients.get(c)
    val snap = me.load.snap
    val queue =
      if (REFETCH_ADVANCES_QUEUE)
        me.q.foldl(List(), (l, o) => l.append(
          if (snap.get(o.todo).exists) o.with("version", snap.get(o.todo).version) else o))
      else me.q
    val draft =
      if (REFETCH_ADVANCES_DRAFT and me.draft.open and snap.get(me.draft.todo).exists)
        me.draft.with("version", snap.get(me.draft.todo).version)
      else me.draft
    all {
      me.load.open,
      clients' = clients.set(c, me.with("conf", snap).with("q", queue).with("draft", draft)
        .with("load", me.load.with("open", false))),
      server' = server, requests' = requests, nextRequest' = nextRequest, nextId' = nextId,
      history' = history, own' = own, submitted' = submitted, saved' = saved,
      offered' = offered, overwrote' = overwrote,
    }
  }

  action init = {
    pure val todos = TODOS.mapBy(_ => FIRST)
    pure val noOp: Op = { kind: "mark", todo: 1, version: 0, title: 0, done: false, seen: 0, id: 0 }
    all {
      server' = todos,
      clients' = CLIENTS.mapBy(_ => { conf: todos, q: List(), request: -1, attempt: 0,
        draft: NO_DRAFT, load: { open: false, snap: todos } }),
      requests' = 0.to(MAX_REQUESTS - 1).mapBy(_ => { client: 1, op: noOp, applied: true,
        answer: { kind: "none", todo: FIRST } }),
      nextRequest' = 0,
      nextId' = 1,
      history' = TODOS.map(t => { todo: t, version: 1, title: 0, done: false }),
      own' = Set(), submitted' = Set(), saved' = Set(), offered' = Set(), overwrote' = false,
    }
  }

  action step = {
    nondet c = CLIENTS.oneOf()
    nondet t = TODOS.oneOf()
    nondet title = TITLES.oneOf()
    nondet r = 0.to(MAX_REQUESTS - 1).oneOf()
    any {
      act(c, t, "mark"), act(c, t, "delete"),
      startDraft(c, t), submitDraft(c, title),
      send(c), apply(r), answer(c), timeOut(c),
      loadRead(c), loadAnswer(c),
    }
  }

  // ── Invariants ──────────────────────────────────────────────────────────

  // Spec: edits made elsewhere are never silently overwritten; late applies too.
  val noSilentOverwrite = not(overwrote)

  // D7: an edit waiting to be sent, or a rename being typed, holds the version
  // the Member saw, advanced only by this tab's own saves.
  val versionsAreSeenOrOwn = CLIENTS.forall(c => {
    val me = clients.get(c)
    and {
      me.q.indices().forall(i => mayEdit(c, me.q[i].todo, me.q[i].seen, me.q[i].version)),
      me.draft.open implies mayEdit(c, me.draft.todo, me.draft.seen, me.draft.version),
    }
  })

  // D7: a submitted text is waiting, saved, or offered again, and only one.
  val noTextIsLost = {
    val waiting = CLIENTS.map(c => {
      val q = clients.get(c).q
      q.indices().filter(i => q[i].kind == "rename").map(i => q[i].id)
    }).flatten()
    and {
      submitted == waiting.union(saved).union(offered),
      waiting.intersect(saved.union(offered)) == Set(),
      saved.intersect(offered) == Set(),
    }
  }

  val safety = and { noSilentOverwrite, versionsAreSeenOrOwn, noTextIsLost }

  // Witnesses: each must be reachable (its negation must have a counterexample).
  val noStale = nextRequest == 0 or 0.to(MAX_REQUESTS - 1).forall(r =>
    requests.get(r).answer.kind != "stale")
  val noGone = 0.to(MAX_REQUESTS - 1).forall(r => requests.get(r).answer.kind != "gone")
  val noRetryAnswered = not(CLIENTS.exists(c =>
    clients.get(c).attempt == 2 and clients.get(c).request != -1 and
      requests.get(clients.get(c).request).applied))
}

// The code as it stands.
module queue {
  import list_queue(TODOS = Set(1), MAX_QUEUE = 2, MAX_REQUESTS = 5,
    REFETCH_ADVANCES_QUEUE = false, REFETCH_ADVANCES_DRAFT = false,
    REBASE_ANY_SUCCESS = false).*
}

// The same with two todos, so that edits of different todos share a queue.
module queue_two_todos {
  import list_queue(TODOS = Set(1, 2), MAX_QUEUE = 3, MAX_REQUESTS = 5,
    REFETCH_ADVANCES_QUEUE = false, REFETCH_ADVANCES_DRAFT = false,
    REBASE_ANY_SUCCESS = false).*
}

module refetch_advances_queue {
  import list_queue(TODOS = Set(1), MAX_QUEUE = 2, MAX_REQUESTS = 5,
    REFETCH_ADVANCES_QUEUE = true, REFETCH_ADVANCES_DRAFT = false,
    REBASE_ANY_SUCCESS = false).*
}

module refetch_advances_draft {
  import list_queue(TODOS = Set(1), MAX_QUEUE = 2, MAX_REQUESTS = 5,
    REFETCH_ADVANCES_QUEUE = false, REFETCH_ADVANCES_DRAFT = true,
    REBASE_ANY_SUCCESS = false).*
}

module rebase_any_success {
  import list_queue(TODOS = Set(1), MAX_QUEUE = 2, MAX_REQUESTS = 5,
    REFETCH_ADVANCES_QUEUE = false, REFETCH_ADVANCES_DRAFT = false,
    REBASE_ANY_SUCCESS = true).*
}
