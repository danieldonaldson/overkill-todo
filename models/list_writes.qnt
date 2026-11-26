// -*- mode: Bluespec; -*-
//
// List and todo writes under concurrency (OT-013 design D4, D6, D10, D13;
// threat model P-35): two Members' lists and todos, written by two concurrent
// requests through backend/crates/persistence-postgres/src/list_repository.rs
// and the purge in user_repository.rs.
//
// A request is three steps:
//
//   send    the request arrives, with all its arguments (chosen freely: any
//           version, count, key, title or neighbour, as a client might send)
//   begin   its transaction takes its lock and reads: the account row
//           (create_list, purge) or the list row (every other write), so it
//           waits while another request holds that lock
//   commit  it decides from what it read, writes, commits and answers
//
// Under the lock, what `begin` read is still current at `commit` for that
// list, so the correct code decides on the current state. The variants that
// drop a lock decide on what `begin` read and write over the current state,
// as Postgres would. The one exception is `create_list`, whose account lock
// doesn't stop a concurrent `delete_list` of the same Member: the code counts
// lists after taking the lock, so the model counts at `commit`, and a deletion
// in between only makes room.
//
//   createList  locks the account (refused if purged: `deleted_at`, the only
//               write that checks it), returns the list with that key, else
//               refuses at MAX_LISTS, else inserts
//   renameList, deleteList (version and todo count), addTodo (key, limit),
//   renameTodo, markTodo, deleteTodo (version), moveTodo (neighbours, D6):
//               lock the Member's list (404 for a missing or foreign one),
//               then select the todo by ID and list
//   purge       anonymize_account: locks the account, then deletes its lists
//               (waiting for any request holding one of them)
//   sweep       the D10 sweep, one statement: deletes lists of purged accounts
//   findList    find_list: two reads, the list row, then its todos, which under
//               READ COMMITTED may be newer than the row. Not replayed.
//
// Bounds: two Members, two requests in flight, MAX_LISTS = 2 with 4 list IDs
// and MAX_TODOS = 3 with 5 todo IDs, so a Member can exceed either limit while
// another Member's rows exist. Keys 1 to 3, names and titles 0 to 2,
// versions 1 to 3. Each limit stands for the real one (100 lists, 1,000 todos):
// the replay fills up to it with rows the model doesn't see. IDs are never
// reused, as UUIDs aren't; a write that needs a new ID when all are used
// answers "bound", changes nothing, and ends the replay of its trace. Positions use the real
// POSITION_GAP; renumbering is reached from adjacent positions in the initial
// states, not by using up a gap. Positions never come near the ends of i64
// here: the Kani proofs in todo_order.rs and todo.rs cover those.
//
// Checks (scripts/models.sh; measured 2026-11-26 on the developer machine):
//   - random simulation (`quint run`), the gate: every invariant over 100,000
//     traces of 30 steps, about 40 s; each broken variant must fail within
//     200,000 (no_lock, the slowest, fails after about 50,000)
//   - Apalache (`quint verify`): every invariant over every execution of 3
//     steps, that is every single request from every initial state, about
//     4 minutes. It took 55 s at 2 steps and 226 s at 3, about four times per
//     step, and two interleaved requests need 6 steps (hours), so races are
//     left to the simulation and to the overlapping-transaction tests in
//     backend/crates/persistence-postgres/tests/lists_integration.rs.
//
// Traces are replayed against PostgresListRepository by
// backend/crates/persistence-postgres/tests/model_conformance.rs (`just models`).
module list_writes {
  // ── Broken variants: each drops one guard (scripts/models.sh) ───────────
  const NO_VERSION_CHECK: bool    // renames, marks and deletes ignore the version
  const NO_COUNT_CHECK: bool      // delete_list ignores the todo count
  const NO_LOCK: bool             // lock_list doesn't lock (`FOR UPDATE` dropped)
  const NO_USER_LOCK: bool        // create_list doesn't lock the account row
  const NO_LIMIT: bool            // neither limit is checked
  const NO_OWNER_CHECK: bool      // lists are looked up by ID alone, also by key
  const NO_KEY_CHECK: bool        // a repeated key isn't looked up
  const NO_DELETED_CHECK: bool    // create_list doesn't check `deleted_at`
  const NO_TODO_LIST_CHECK: bool  // todos are selected by ID alone
  const NO_NEIGHBOUR_SCOPE: bool  // a move's neighbours aren't scoped to the list

  pure val MAX_LISTS = 2
  pure val MAX_TODOS = 3
  // POSITION_GAP in backend/crates/domain/src/entities/todo.rs.
  pure val GAP = 1048576

  pure val MEMBERS = Set(1, 2)
  pure val WRITERS = Set(1, 2)
  pure val LIST_IDS = Set(1, 2, 3, 4)
  pure val TODO_IDS = Set(1, 2, 3, 4, 5)
  pure val KEYS = Set(1, 2, 3)
  pure val TEXTS = Set(0, 1, 2)
  pure val VERSIONS = Set(1, 2, 3)

  // `used`: the ID has been taken (IDs are never reused). `exists`: still stored.
  type ListRow = { used: bool, exists: bool, owner: int, key: int, name: int, version: int }
  // Open todos have a position; done ones have `done` and position 0.
  type TodoRow = { used: bool, exists: bool, list: int, key: int, title: int,
                   version: int, done: bool, pos: int }

  // `op` is "" while idle. Arguments not used by `op` are 0. `above`/`below`
  // are a move's neighbour lists of at most one ID: 0 for an empty list.
  // `pc`: "idle", "sent", "held" (begun: holds its lock), "read" (findList
  // between its two reads). `seenLists`/`seenTodos`: what `begin` read.
  type Writer = { pc: str, op: str, member: int, list: int, todo: int, key: int,
                  version: int, text: int, done: bool, count: int, above: int,
                  below: int, seenLists: int -> ListRow, seenTodos: int -> TodoRow,
                  found: bool }

  // The step just taken, for the invariants and the replay. `outcome` for a
  // commit: "applied", "repeated", "stale", "listLimit", "todoLimit",
  // "listNotFound", "todoNotFound", "accountGone", "moved" (`renumbered` if
  // the list was renumbered), "purged", "notDue", "found" (findList), "swept",
  // "bound" (no free ID: the end of the bound);
  // "" otherwise. `id`: the row answered (a todo or list), the Member purged,
  // or 0. The rest are ghosts.
  type Last = {
    act: str, writer: int, op: str, outcome: str, id: int, renumbered: bool,
    // Rename, mark or delete: the version sent differed from the stored one,
    // and the state asked for wasn't already stored.
    staleRequest: bool,
    // Stored rows changed in this step.
    changed: bool,
    // The version of the row named, before and after.
    versionBefore: int, versionAfter: int,
    // Another Member's row was changed, or answered (also a neighbour ID).
    touchedOther: bool, revealedOther: bool,
    // delete_list: the count sent differed from the stored one.
    countChanged: bool,
    // A move changed which todos exist, or which are open.
    moveChangedTodos: bool,
    // create_list stored a list for an account already purged.
    listForPurged: bool,
  }

  var deleted: int -> bool
  var lists: int -> ListRow
  var todos: int -> TodoRow
  var writers: int -> Writer
  var last: Last

  pure val NO_LIST: ListRow = { used: false, exists: false, owner: 0, key: 0, name: 0, version: 0 }
  pure val NO_TODO: TodoRow = { used: false, exists: false, list: 0, key: 0, title: 0,
                                version: 0, done: false, pos: 0 }

  pure def listRow(owner: int, key: int, name: int, version: int): ListRow =
    { used: true, exists: true, owner: owner, key: key, name: name, version: version }
  pure def openTodo(list: int, key: int, version: int, pos: int): TodoRow =
    { used: true, exists: true, list: list, key: key, title: 0, version: version,
      done: false, pos: pos }
  pure def doneTodo(list: int, key: int, version: int): TodoRow =
    { used: true, exists: true, list: list, key: key, title: 0, version: version,
      done: true, pos: 0 }

  pure def idle(seenLists: int -> ListRow, seenTodos: int -> TodoRow): Writer =
    { pc: "idle", op: "", member: 0, list: 0, todo: 0, key: 0, version: 0, text: 0,
      done: false, count: 0, above: 0, below: 0, seenLists: seenLists,
      seenTodos: seenTodos, found: false }

  pure val NOTHING: Last = {
    act: "", writer: 0, op: "", outcome: "", id: 0, renumbered: false,
    staleRequest: false, changed: false, versionBefore: 0, versionAfter: 0,
    touchedOther: false, revealedOther: false, countChanged: false,
    moveChangedTodos: false, listForPurged: false,
  }

  // ── Initial states ──────────────────────────────────────────────────────
  //
  // Populated states that meet the table constraints, since 8 steps from empty
  // reach neither a full list nor two concurrent moves.

  type Seed = { deleted: int -> bool, lists: int -> ListRow, todos: int -> TodoRow }

  // Keys run from 1 to 3 in requests; key 4 marks a row no request repeats,
  // so a full list can also be added to.
  //
  // At the list limit (Member 1) and the todo limit (list 1, three open todos,
  // two of them adjacent, so moving the third between them renumbers);
  // Member 2 below both, with a list key Member 1 also uses.
  pure val AT_LIMITS: Seed = {
    deleted: MEMBERS.mapBy(_ => false),
    lists: Map(1 -> listRow(1, 1, 0, 1), 2 -> listRow(1, 2, 0, 2),
               3 -> listRow(2, 1, 0, 1), 4 -> NO_LIST),
    todos: Map(1 -> openTodo(1, 1, 1, GAP), 2 -> openTodo(1, 2, 2, GAP + 1),
               3 -> openTodo(1, 4, 1, 3 * GAP), 4 -> openTodo(3, 1, 1, GAP), 5 -> NO_TODO),
  }

  // Just below both limits (Member 1: one list, list 1 with an open and a done
  // todo), with room for two more lists and two more todos.
  pure val BELOW_LIMITS: Seed = {
    deleted: MEMBERS.mapBy(_ => false),
    lists: Map(1 -> listRow(1, 1, 0, 1), 2 -> listRow(2, 1, 0, 1),
               3 -> NO_LIST, 4 -> NO_LIST),
    todos: Map(1 -> openTodo(1, 1, 1, GAP), 2 -> doneTodo(1, 2, 2),
               3 -> openTodo(2, 1, 1, GAP), 4 -> NO_TODO, 5 -> NO_TODO),
  }

  // Member 2 purged by the previous release, with a list left behind for the
  // D10 sweep; Member 1 with a full list of three open todos.
  pure val LEFT_BEHIND: Seed = {
    deleted: Map(1 -> false, 2 -> true),
    lists: Map(1 -> listRow(1, 1, 0, 1), 2 -> NO_LIST,
               3 -> listRow(2, 1, 0, 1), 4 -> NO_LIST),
    todos: Map(1 -> openTodo(1, 1, 1, GAP), 2 -> openTodo(1, 2, 1, 2 * GAP),
               3 -> openTodo(1, 4, 1, 3 * GAP), 4 -> openTodo(3, 1, 1, GAP),
               5 -> NO_TODO),
  }

  pure val SEEDS = Set(AT_LIMITS, BELOW_LIMITS, LEFT_BEHIND)

  // ── Reading ─────────────────────────────────────────────────────────────

  pure def ownerOf(ls: int -> ListRow, ts: int -> TodoRow, t: int): int =
    ls.get(ts.get(t).list).owner

  // lock_list's `WHERE id = $1 AND user_id = $2`.
  pure def ownsList(ls: int -> ListRow, m: int, l: int): bool =
    LIST_IDS.contains(l) and ls.get(l).exists and (NO_OWNER_CHECK or ls.get(l).owner == m)

  // todo_row's `WHERE id = $1 AND list_id = $2`.
  pure def inList(ts: int -> TodoRow, l: int, t: int): bool =
    ts.get(t).exists and (NO_TODO_LIST_CHECK or ts.get(t).list == l)

  pure def listCount(ls: int -> ListRow, m: int): int =
    LIST_IDS.filter(l => ls.get(l).exists and ls.get(l).owner == m).size()

  pure def todoCount(ts: int -> TodoRow, l: int): int =
    TODO_IDS.filter(t => ts.get(t).exists and ts.get(t).list == l).size()

  pure def openIn(ts: int -> TodoRow, l: int): Set[int] =
    TODO_IDS.filter(t => ts.get(t).exists and not(ts.get(t).done) and ts.get(t).list == l)

  // append_position(MAX(position) of the list's open todos).
  pure def appendPosition(ts: int -> TodoRow, l: int): int =
    openIn(ts, l).fold(GAP, (acc, t) => if (ts.get(t).pos + GAP > acc) ts.get(t).pos + GAP else acc)

  // An ID of a list never used, or 0.
  pure def freeListId(ls: int -> ListRow): Set[int] = LIST_IDS.filter(l => not(ls.get(l).used))
  pure def freeTodoId(ts: int -> TodoRow): Set[int] = TODO_IDS.filter(t => not(ts.get(t).used))

  // Deletes lists and, by ON DELETE CASCADE, their todos.
  pure def dropLists(ls: int -> ListRow, ts: int -> TodoRow, gone: Set[int]): (int -> ListRow, int -> TodoRow) =
    (LIST_IDS.mapBy(l => if (gone.contains(l)) ls.get(l).with("exists", false) else ls.get(l)),
     TODO_IDS.mapBy(t => if (gone.contains(ts.get(t).list)) ts.get(t).with("exists", false) else ts.get(t)))

  // ── Ordering (D6, domain todo_order.rs) ─────────────────────────────────

  // i64::midpoint: rounds towards zero.
  pure def midpoint(a: int, b: int): int =
    if (a + b >= 0) (a + b) / 2 else -((-(a + b)) / 2)

  // The highest position in `others` (below `bound`, if `strict`), if any.
  pure def highest(ts: int -> TodoRow, others: Set[int], bound: int, strict: bool): { has: bool, at: int } =
    others.fold({ has: false, at: 0 }, (acc, t) =>
      if ((not(strict) or ts.get(t).pos < bound) and (not(acc.has) or ts.get(t).pos > acc.at))
        { has: true, at: ts.get(t).pos } else acc)
  // The lowest position in `others` (above `bound`, if `strict`), if any.
  pure def lowest(ts: int -> TodoRow, others: Set[int], bound: int, strict: bool): { has: bool, at: int } =
    others.fold({ has: false, at: 0 }, (acc, t) =>
      if ((not(strict) or ts.get(t).pos > bound) and (not(acc.has) or ts.get(t).pos < acc.at))
        { has: true, at: ts.get(t).pos } else acc)

  type Placement = { hasAbove: bool, above: int, hasBelow: bool, below: int, slot: int }

  // move_slot, then the positions directly above and below the slot.
  // `others`: the open todos but the moved one; `aboveId`, `belowId`: 0 for an
  // empty neighbour list.
  pure def place(ts: int -> TodoRow, others: Set[int], aboveId: int, belowId: int): Placement = {
    // Below the first remaining ID in `above`, else above the first remaining
    // one in `below`, else the bottom; an `above` sent empty means the top.
    val a =
      if (aboveId == 0) { has: false, at: 0 }
      else if (others.contains(aboveId)) { has: true, at: ts.get(aboveId).pos }
      else if (belowId != 0 and others.contains(belowId)) highest(ts, others, ts.get(belowId).pos, true)
      else highest(ts, others, 0, false)
    val b =
      if (aboveId == 0) lowest(ts, others, 0, false)
      else if (others.contains(aboveId)) lowest(ts, others, ts.get(aboveId).pos, true)
      else if (belowId != 0 and others.contains(belowId)) { has: true, at: ts.get(belowId).pos }
      else { has: false, at: 0 }
    { hasAbove: a.has, above: a.at, hasBelow: b.has, below: b.at,
      slot: if (a.has) others.filter(t => ts.get(t).pos <= a.at).size() else 0 }
  }

  // position_between: a position strictly between, or none (renumber).
  pure def between(p: Placement): { ok: bool, at: int } =
    if (not(p.hasAbove) and not(p.hasBelow)) { ok: true, at: GAP }
    else if (not(p.hasBelow)) { ok: true, at: p.above + GAP }
    else if (not(p.hasAbove)) { ok: true, at: p.below - GAP }
    else {
      val m = midpoint(p.above, p.below)
      { ok: p.above < m and m < p.below, at: m }
    }

  // A todo's rank among `others` by position.
  pure def rankIn(ts: int -> TodoRow, others: Set[int], x: int): int =
    others.filter(y => ts.get(y).pos < ts.get(x).pos).size()

  // ── Requests ────────────────────────────────────────────────────────────

  pure val LIST_OPS = Set("renameList", "deleteList", "addTodo", "renameTodo", "markTodo",
                          "deleteTodo", "moveTodo")
  pure val USER_OPS = Set("createList", "purge")
  // The writes that send a version.
  pure val VERSIONED = Set("renameList", "deleteList", "renameTodo", "markTodo", "deleteTodo")

  // The lock `w` holds, if it has begun: an account (createList, purge) or a list.
  def holdsUser(w: int, m: int): bool = {
    val me = writers.get(w)
    me.pc == "held" and me.member == m and
      (me.op == "purge" or (me.op == "createList" and not(NO_USER_LOCK)))
  }
  def holdsList(w: int, l: int): bool = {
    val me = writers.get(w)
    me.pc == "held" and me.list == l and LIST_OPS.contains(me.op) and not(NO_LOCK)
  }

  action unchangedRows = all { deleted' = deleted, lists' = lists, todos' = todos }

  action send(w: int, req: Writer): bool = all {
    writers.get(w).pc == "idle",
    writers' = writers.set(w, req.with("pc", "sent")),
    unchangedRows,
    last' = NOTHING.with("act", "send").with("writer", w).with("op", req.op),
  }

  // Takes the lock once no other request holds it, and reads.
  action begin(w: int): bool = {
    val me = writers.get(w)
    val free =
      if (USER_OPS.contains(me.op)) WRITERS.forall(o => o == w or not(holdsUser(o, me.member)))
      else if (LIST_OPS.contains(me.op))
        // A list row that doesn't match locks nothing; one that matches waits.
        WRITERS.forall(o => o == w or not(holdsList(o, me.list)))
      else true
    all {
      me.pc == "sent",
      free,
      writers' = writers.set(w, me.with("pc", if (me.op == "findList") "read" else "held")
        .with("seenLists", lists).with("seenTodos", todos)
        // findList's first read: the list row, if it is the Member's.
        .with("found", ownsList(lists, me.member, me.list))),
      unchangedRows,
      last' = NOTHING.with("act", "begin").with("writer", w).with("op", me.op),
    }
  }

  // The result of a commit: the rows written and what the step reports.
  type Result = { ok: bool, lists: int -> ListRow, todos: int -> TodoRow, deleted: int -> bool, last: Last }

  pure def listOwnerIn(ls: int -> ListRow, nl: int -> ListRow, l: int): int =
    if (nl.get(l).used) nl.get(l).owner else ls.get(l).owner
  pure def todoOwnerIn(ls: int -> ListRow, ts: int -> TodoRow, nl: int -> ListRow,
                       nt: int -> TodoRow, t: int): int =
    listOwnerIn(ls, nl, if (nt.get(t).used) nt.get(t).list else ts.get(t).list)

  // Who owns what was changed or answered.
  pure def ownership(ls: int -> ListRow, ts: int -> TodoRow, nl: int -> ListRow,
                     nt: int -> TodoRow, m: int, answered: Set[int], answeredLists: Set[int]): { touched: bool, revealed: bool } = {
    {
      touched: LIST_IDS.exists(l => nl.get(l) != ls.get(l) and listOwnerIn(ls, nl, l) != m) or
               TODO_IDS.exists(t => nt.get(t) != ts.get(t) and todoOwnerIn(ls, ts, nl, nt, t) != m),
      revealed: answered.exists(t => todoOwnerIn(ls, ts, nl, nt, t) != m) or answeredLists.exists(l => listOwnerIn(ls, nl, l) != m),
    }
  }

  // Fills in what a commit reports: its outcome, the row answered, and whose
  // rows it changed or answered. `cl`/`ct`: the rows before the commit.
  pure def report(cl: int -> ListRow, ct: int -> TodoRow, m: int, r: Result, outcome: str,
                  id: int, answered: Set[int], answeredLists: Set[int]): Result = {
    val o = ownership(cl, ct, r.lists, r.todos, m, answered, answeredLists)
    r.with("last", r.last.with("outcome", outcome).with("id", id)
      .with("changed", r.lists != cl or r.todos != ct)
      .with("touchedOther", o.touched).with("revealedOther", o.revealed))
  }

  // A list or todo write by Member `m`, deciding on `ls`/`ts` (what it read,
  // which under its lock is the current state) and writing over `cl`/`ct`.
  pure def listWrite(me: Writer, ls: int -> ListRow, ts: int -> TodoRow,
                     cl: int -> ListRow, ct: int -> TodoRow, d: int -> bool): Result = {
    val m = me.member
    val l = me.list
    val t = me.todo
    val base = NOTHING.with("act", "commit").with("op", me.op)
    val keep = { ok: true, lists: cl, todos: ct, deleted: d, last: base }
    if (not(ownsList(ls, m, l))) report(cl, ct, m, keep, "listNotFound", 0, Set(), Set())
    else if (me.op == "renameList") {
      val row = ls.get(l)
      val already = row.name == me.text
      val stale = row.version != me.version and not(NO_VERSION_CHECK)
      val vb = base.with("versionBefore", row.version)
        .with("staleRequest", row.version != me.version and not(already))
      if (already) report(cl, ct, m, keep.with("last", vb.with("versionAfter", row.version)), "repeated", l, Set(), Set(l))
      else if (stale) report(cl, ct, m, keep.with("last", vb.with("versionAfter", row.version)), "stale", l, Set(), Set(l))
      else {
        val nl = cl.set(l, cl.get(l).with("name", me.text).with("version", cl.get(l).version + 1))
        report(cl, ct, m, keep.with("lists", nl).with("last", vb.with("versionAfter", nl.get(l).version)),
               "applied", l, Set(), Set(l))
      }
    }
    else if (me.op == "deleteList") {
      val row = ls.get(l)
      val countChanged = todoCount(ts, l) != me.count
      val stale = (row.version != me.version and not(NO_VERSION_CHECK)) or
                  (countChanged and not(NO_COUNT_CHECK))
      val vb = base.with("versionBefore", row.version).with("versionAfter", row.version)
        .with("staleRequest", row.version != me.version).with("countChanged", countChanged)
      if (stale) report(cl, ct, m, keep.with("last", vb), "stale", l, Set(), Set(l))
      else {
        val dropped = dropLists(cl, ct, Set(l))
        report(cl, ct, m, keep.with("lists", dropped._1).with("todos", dropped._2).with("last", vb),
               "applied", l, Set(), Set(l))
      }
    }
    else if (me.op == "addTodo") {
      val repeat = TODO_IDS.filter(x => ts.get(x).exists and ts.get(x).list == l and ts.get(x).key == me.key)
      if (not(NO_KEY_CHECK) and repeat != Set())
        repeat.fold(keep, (_acc, x) => report(cl, ct, m, keep.with("last", base.with("versionBefore", ts.get(x).version)
          .with("versionAfter", ts.get(x).version)), "repeated", x, Set(x), Set()))
      else if (todoCount(ts, l) >= MAX_TODOS and not(NO_LIMIT)) report(cl, ct, m, keep, "todoLimit", 0, Set(), Set())
      else {
        val free = freeTodoId(ct)
        // Every todo ID used: the end of the bound, not a behaviour.
        if (free == Set()) keep.with("last", base.with("outcome", "bound"))
        else free.fold(keep.with("ok", false), (acc, x) =>
          if (acc.ok) acc
          else {
            val nt = ct.set(x, { used: true, exists: true, list: l, key: me.key, title: me.text,
                                 version: 1, done: false, pos: appendPosition(ts, l) })
            report(cl, ct, m, keep.with("todos", nt).with("last", base.with("versionAfter", 1)), "applied", x, Set(x), Set())
          })
      }
    }
    else if (not(inList(ts, l, t))) report(cl, ct, m, keep, "todoNotFound", 0, Set(), Set())
    else if (me.op == "renameTodo" or me.op == "markTodo") {
      val row = ts.get(t)
      val already = if (me.op == "renameTodo") row.title == me.text else row.done == me.done
      val stale = row.version != me.version and not(NO_VERSION_CHECK)
      val vb = base.with("versionBefore", row.version)
        .with("staleRequest", row.version != me.version and not(already))
      if (already) report(cl, ct, m, keep.with("last", vb.with("versionAfter", row.version)), "repeated", t, Set(t), Set())
      else if (stale) report(cl, ct, m, keep.with("last", vb.with("versionAfter", row.version)), "stale", t, Set(t), Set())
      else {
        val cur = ct.get(t)
        val written =
          if (me.op == "renameTodo") cur.with("title", me.text)
          else if (me.done) cur.with("done", true).with("pos", 0)
          // Marked not done: to the bottom of the open todos.
          else cur.with("done", false).with("pos", appendPosition(ts, l))
        val nt = ct.set(t, written.with("version", cur.version + 1))
        report(cl, ct, m, keep.with("todos", nt).with("last", vb.with("versionAfter", nt.get(t).version)),
               "applied", t, Set(t), Set())
      }
    }
    else if (me.op == "deleteTodo") {
      val row = ts.get(t)
      val stale = row.version != me.version and not(NO_VERSION_CHECK)
      val vb = base.with("versionBefore", row.version).with("versionAfter", row.version)
        .with("staleRequest", row.version != me.version)
      if (stale) report(cl, ct, m, keep.with("last", vb), "stale", t, Set(t), Set())
      else report(cl, ct, m, keep.with("todos", ct.set(t, ct.get(t).with("exists", false))).with("last", vb),
                  "applied", t, Set(t), Set())
    }
    else {
      // moveTodo. A done todo answers stale; versions never change.
      val row = ts.get(t)
      val vb = base.with("versionBefore", row.version).with("versionAfter", row.version)
      if (row.done) report(cl, ct, m, keep.with("last", vb), "stale", t, Set(t), Set())
      else {
        val others =
          if (NO_NEIGHBOUR_SCOPE)
            TODO_IDS.filter(x => x != t and ts.get(x).exists and not(ts.get(x).done))
          else openIn(ts, l).exclude(Set(t))
        val p = place(ts, others, me.above, me.below)
        val inPlace = (not(p.hasAbove) or p.above < row.pos) and (not(p.hasBelow) or row.pos < p.below)
        val mid = between(p)
        // renumber: `UPDATE … WHERE todos.list_id = $1`, so only this list's rows.
        val renumbered = TODO_IDS.mapBy(x =>
          if (x == t) ct.get(x).with("pos", (p.slot + 1) * GAP)
          else if (others.contains(x) and ct.get(x).list == l and ct.get(x).exists)
            ct.get(x).with("pos", (rankIn(ts, others, x) + (if (rankIn(ts, others, x) >= p.slot) 1 else 0) + 1) * GAP)
          else ct.get(x))
        val nt =
          if (inPlace) ct
          else if (mid.ok) ct.set(t, ct.get(t).with("pos", mid.at))
          else renumbered
        val membership = TODO_IDS.exists(x =>
          nt.get(x).exists != ct.get(x).exists or nt.get(x).done != ct.get(x).done)
        val versionsMoved = TODO_IDS.exists(x => nt.get(x).version != ct.get(x).version)
        report(cl, ct, m, keep.with("todos", nt).with("last", vb.with("renumbered", not(inPlace) and not(mid.ok))
                 .with("moveChangedTodos", membership or versionsMoved)),
               "moved", t, others.union(Set(t)), Set())
      }
    }
  }

  // create_list for Member `m`: `ls` is what it read.
  // `purgedThen`: the account was purged when it was read; `purgedNow`: at commit.
  pure def createList(me: Writer, ls: int -> ListRow, cl: int -> ListRow, ct: int -> TodoRow,
                      d: int -> bool, purgedThen: bool, purgedNow: bool): Result = {
    val m = me.member
    val base = NOTHING.with("act", "commit").with("op", me.op)
    val keep = { ok: true, lists: cl, todos: ct, deleted: d, last: base }
    // The key lookup is scoped to the Member (`WHERE user_id = $1 AND
    // creation_key = $2`); without the owner check it isn't.
    val repeat = LIST_IDS.filter(l => ls.get(l).exists and ls.get(l).key == me.key and
                                      (NO_OWNER_CHECK or ls.get(l).owner == m))
    if (purgedThen and not(NO_DELETED_CHECK)) keep.with("last", base.with("outcome", "accountGone"))
    else if (not(NO_KEY_CHECK) and repeat != Set())
      repeat.fold(keep, (_acc, l) => keep.with("last", base.with("outcome", "repeated").with("id", l)
        .with("versionBefore", ls.get(l).version).with("versionAfter", ls.get(l).version)
        .with("revealedOther", ls.get(l).owner != m)))
    else if (listCount(ls, m) >= MAX_LISTS and not(NO_LIMIT)) keep.with("last", base.with("outcome", "listLimit"))
    else {
      val free = freeListId(cl)
      // Every list ID used: the end of the bound, not a behaviour.
      if (free == Set()) keep.with("last", base.with("outcome", "bound"))
      else free.fold(keep.with("ok", false), (acc, l) =>
        if (acc.ok) acc
        else {
          val nl = cl.set(l, listRow(m, me.key, me.text, 1))
          keep.with("lists", nl).with("last", base.with("outcome", "applied").with("id", l)
            .with("changed", true).with("versionAfter", 1).with("listForPurged", purgedNow))
        })
    }
  }

  action commit(w: int): bool = {
    val me = writers.get(w)
    val m = me.member
    val r =
      if (me.op == "createList") {
        // With the account lock, the account and its lists are read now (see
        // the top); without it, at `begin`, so a purge can slip in between.
        val ls = if (NO_USER_LOCK) me.seenLists else lists
        val purgedThen = if (NO_USER_LOCK) me.found else deleted.get(m)
        createList(me, ls, lists, todos, deleted, purgedThen, deleted.get(m))
      }
      else if (me.op == "purge") {
        // anonymize_account: refused (no longer due) if already purged; else
        // marks the account deleted and deletes its lists, in one transaction.
        val mine = LIST_IDS.filter(l => lists.get(l).exists and lists.get(l).owner == m)
        val dropped = dropLists(lists, todos, mine)
        val base = NOTHING.with("act", "commit").with("op", "purge")
        if (deleted.get(m))
          { ok: true, lists: lists, todos: todos, deleted: deleted, last: base.with("outcome", "notDue").with("id", m) }
        else
          { ok: true, lists: dropped._1, todos: dropped._2, deleted: deleted.set(m, true),
            last: base.with("outcome", "purged").with("id", m).with("changed", true) }
      }
      else if (NO_LOCK) listWrite(me, me.seenLists, me.seenTodos, lists, todos, deleted)
      else listWrite(me, lists, todos, lists, todos, deleted)
    // The purge's DELETE waits for every request holding one of its lists.
    val purgeWaits = me.op == "purge" and WRITERS.exists(o => o != w and
      LIST_IDS.exists(l => lists.get(l).exists and lists.get(l).owner == m and holdsList(o, l)))
    all {
      me.pc == "held",
      r.ok,
      not(purgeWaits),
      lists' = r.lists, todos' = r.todos, deleted' = r.deleted,
      writers' = writers.set(w, idle(lists, todos)),
      last' = r.last.with("writer", w),
    }
  }

  // createList without the account lock: remembers at `begin` whether the
  // account was purged then.
  action beginCreateUnlocked(w: int): bool = {
    val me = writers.get(w)
    all {
      NO_USER_LOCK,
      me.pc == "sent",
      me.op == "createList",
      writers' = writers.set(w, me.with("pc", "held").with("seenLists", lists)
        .with("seenTodos", todos).with("found", deleted.get(me.member))),
      unchangedRows,
      last' = NOTHING.with("act", "begin").with("writer", w).with("op", me.op),
    }
  }

  // find_list's second read: the list's todos, by list ID alone.
  action readTodos(w: int): bool = {
    val me = writers.get(w)
    val answered = if (me.found) TODO_IDS.filter(t => todos.get(t).exists and todos.get(t).list == me.list) else Set()
    val listOwner = me.seenLists.get(me.list).owner
    all {
      me.pc == "read",
      writers' = writers.set(w, idle(lists, todos)),
      unchangedRows,
      last' = NOTHING.with("act", "commit").with("writer", w).with("op", "findList")
        .with("outcome", if (me.found) "found" else "listNotFound")
        .with("revealedOther", me.found and (listOwner != me.member or
          answered.exists(t => ownerOf(lists, todos, t) != me.member))),
    }
  }

  // The D10 sweep: one statement, waiting for any request holding a list.
  action sweep: bool = {
    val gone = LIST_IDS.filter(l => lists.get(l).exists and deleted.get(lists.get(l).owner))
    val dropped = dropLists(lists, todos, gone)
    all {
      // A sweep with nothing to delete changes nothing, so it isn't a step.
      gone != Set(),
      WRITERS.forall(o => gone.forall(l => not(holdsList(o, l)))),
      lists' = dropped._1, todos' = dropped._2, deleted' = deleted,
      writers' = writers,
      last' = NOTHING.with("act", "sweep").with("outcome", "swept").with("changed", true),
    }
  }

  action init = {
    nondet seed = SEEDS.oneOf()
    all {
      deleted' = seed.deleted,
      lists' = seed.lists,
      todos' = seed.todos,
      writers' = WRITERS.mapBy(_ => idle(seed.lists, seed.todos)),
      last' = NOTHING.with("act", "init"),
    }
  }

  // A request with freely chosen arguments. Its list is usually that of its
  // todo, and its Member usually the list's owner, so that most requests get
  // past the lookups; `stray` and `foreign` choose any list and any Member.
  action sendAny(w: int): bool = all {
    // Checked before anything is chosen: the simulator tries the choices of
    // a disabled action one by one.
    writers.get(w).pc == "idle",
    sendWith(w),
  }

  action sendWith(w: int): bool = {
    nondet op = LIST_OPS.union(USER_OPS).union(Set("findList")).oneOf()
    nondet t = TODO_IDS.oneOf()
    nondet stray = Set(true, false).oneOf()
    nondet anyList = LIST_IDS.oneOf()
    nondet foreign = Set(true, false).oneOf()
    nondet anyMember = MEMBERS.oneOf()
    nondet key = KEYS.oneOf()
    nondet version = VERSIONS.oneOf()
    nondet text = TEXTS.oneOf()
    nondet done = Set(true, false).oneOf()
    nondet count = 0.to(MAX_TODOS).oneOf()
    nondet above = TODO_IDS.union(Set(0)).oneOf()
    nondet below = TODO_IDS.union(Set(0)).oneOf()
    val l = if (stray or not(todos.get(t).used)) anyList else todos.get(t).list
    val m = if (foreign or not(lists.get(l).used)) anyMember else lists.get(l).owner
    // Only the arguments `op` uses, so equal requests are equal records.
    val req = writers.get(w).with("op", op).with("member", m)
      .with("list", if (op == "createList" or op == "purge") 0 else l)
      .with("todo", if (Set("renameTodo", "markTodo", "deleteTodo", "moveTodo").contains(op)) t else 0)
      .with("key", if (op == "createList" or op == "addTodo") key else 0)
      .with("version", if (VERSIONED.contains(op)) version else 0)
      .with("text", if (Set("createList", "renameList", "addTodo", "renameTodo").contains(op)) text else 0)
      .with("done", op == "markTodo" and done)
      .with("count", if (op == "deleteList") count else 0)
      .with("above", if (op == "moveTodo") above else 0)
      .with("below", if (op == "moveTodo") below else 0)
    send(w, req)
  }

  // The arguments are chosen inside `sendAny`, so a step that only begins,
  // commits or reads doesn't choose them.
  action step = {
    nondet w = WRITERS.oneOf()
    any {
      sendAny(w),
      if (NO_USER_LOCK and writers.get(w).op == "createList") beginCreateUnlocked(w) else begin(w),
      commit(w),
      readTodos(w),
      sweep,
    }
  }

  // ── Invariants ──────────────────────────────────────────────────────────

  // Spec (lists): edits made elsewhere are never silently overwritten. A
  // rename, mark or delete sent with a version other than the stored one
  // changes nothing, unless what it asks for is already stored (which changes
  // nothing either).
  val staleChangesNothing =
    (last.act == "commit" and VERSIONED.contains(last.op) and last.staleRequest) implies
      (not(last.changed) and last.outcome == "stale")

  // D1: an applied rename or mark moves the version by one; a request for
  // what is stored leaves it.
  val versionsMoveByOne =
    (last.act == "commit" and Set("renameList", "renameTodo", "markTodo").contains(last.op)) implies
      (if (last.outcome == "applied") last.versionAfter == last.versionBefore + 1
       else last.versionAfter == last.versionBefore)

  // D6: a move neither checks nor changes a version, and leaves every todo
  // there and open or done as it was.
  val movesKeepEveryTodoOnce =
    (last.act == "commit" and last.op == "moveTodo") implies not(last.moveChangedTodos)

  // Spec (lists): a repeated save takes effect once. One stored row per key,
  // per Member for lists and per list for todos.
  val repeatStoresOneRow = and {
    LIST_IDS.forall(a => LIST_IDS.forall(b =>
      (a != b and lists.get(a).exists and lists.get(b).exists and
        lists.get(a).owner == lists.get(b).owner) implies lists.get(a).key != lists.get(b).key)),
    TODO_IDS.forall(a => TODO_IDS.forall(b =>
      (a != b and todos.get(a).exists and todos.get(b).exists and
        todos.get(a).list == todos.get(b).list) implies todos.get(a).key != todos.get(b).key)),
  }

  // Spec (lists): lists and todos have limits, under concurrent requests.
  val listLimitHolds = MEMBERS.forall(m => listCount(lists, m) <= MAX_LISTS)
  val todoLimitHolds = LIST_IDS.forall(l => todoCount(todos, l) <= MAX_TODOS)

  // Spec (lists): lists follow the account through deletion. No list is
  // created for a purged account, and none is left once its purge, or the
  // sweep after a purge by the previous release, has run.
  val noListForPurgedAccount = not(last.listForPurged)
  val purgeLeavesNoLists = and {
    (last.op == "purge" and last.outcome == "purged") implies
      LIST_IDS.forall(l => lists.get(l).exists implies lists.get(l).owner != last.id),
    last.act == "sweep" implies
      LIST_IDS.forall(l => lists.get(l).exists implies not(deleted.get(lists.get(l).owner))),
  }

  // Spec (security, P-35): a Member's writes and reads never change or answer
  // another Member's rows: not by list, by todo, by key or by neighbour.
  val writesStayWithTheirMember =
    (last.act == "commit" and last.op != "findList") implies not(last.touchedOther or last.revealedOther)
  // find_list's two reads never answer another Member's list or todos.
  val findListStaysWithItsMember =
    (last.act == "commit" and last.op == "findList") implies not(last.revealedOther)

  // Spec (lists): deleting a list deletes its todos, and is refused if the
  // number of todos changed since it was confirmed.
  val noTodoOutlivesItsList =
    TODO_IDS.forall(t => todos.get(t).exists implies lists.get(todos.get(t).list).exists)
  val listDeleteRefusesChangedCount =
    (last.act == "commit" and last.op == "deleteList" and last.countChanged) implies not(last.changed)

  // D6: open todos in a list have distinct positions, so the list has one order.
  val openPositionsAreUnique =
    TODO_IDS.forall(a => TODO_IDS.forall(b =>
      (a != b and todos.get(a).exists and todos.get(b).exists and
        not(todos.get(a).done) and not(todos.get(b).done) and
        todos.get(a).list == todos.get(b).list) implies todos.get(a).pos != todos.get(b).pos))

  val safety = and {
    staleChangesNothing, versionsMoveByOne, movesKeepEveryTodoOnce, repeatStoresOneRow,
    listLimitHolds, todoLimitHolds, noListForPurgedAccount, purgeLeavesNoLists,
    writesStayWithTheirMember, findListStaysWithItsMember, noTodoOutlivesItsList,
    listDeleteRefusesChangedCount, openPositionsAreUnique,
  }

  // ── Witnesses: each must be reachable (its negation has a counterexample) ─

  // For the replay as well as the bound: traces ending in a rare outcome.
  val noStale = last.outcome != "stale"
  val noRepeat = last.outcome != "repeated" or not(Set("createList", "addTodo").contains(last.op))
  val noListNotFound = last.outcome != "listNotFound" or last.op == "findList"
  val noTodoNotFound = last.outcome != "todoNotFound"
  val noPurge = last.outcome != "purged"
  val noSweep = last.act != "sweep"
  val noListLimit = last.outcome != "listLimit"
  val noTodoLimit = last.outcome != "todoLimit"
  val noRenumbering = not(last.renumbered)
  // Two moves of the same list in flight at once, one holding the lock.
  val noOverlappingMoves = not(WRITERS.exists(a => WRITERS.exists(b =>
    a != b and writers.get(a).op == "moveTodo" and writers.get(b).op == "moveTodo" and
      writers.get(a).list == writers.get(b).list and
      writers.get(a).pc == "held" and writers.get(b).pc == "sent")))
}

// The code as it stands.
module writes {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false,
    NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false,
    NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

// One module per guard, each with that guard dropped: scripts/models.sh
// requires a counterexample from each.

module no_version_check {
  import list_writes(NO_VERSION_CHECK = true, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_count_check {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = true, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_lock {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = true, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_user_lock {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = true, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_limit {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = true, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_owner_check {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = true, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_key_check {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = true, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_deleted_check {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = true, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = false).*
}

module no_todo_list_check {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = true, NO_NEIGHBOUR_SCOPE = false).*
}

module no_neighbour_scope {
  import list_writes(NO_VERSION_CHECK = false, NO_COUNT_CHECK = false, NO_LOCK = false, NO_USER_LOCK = false, NO_LIMIT = false, NO_OWNER_CHECK = false, NO_KEY_CHECK = false, NO_DELETED_CHECK = false, NO_TODO_LIST_CHECK = false, NO_NEIGHBOUR_SCOPE = true).*
}
