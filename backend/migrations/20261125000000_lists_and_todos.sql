-- Todo lists and the todos inside them (OT-013 design D2).
--
-- Both tables are additive: the previous release neither reads nor writes them,
-- so a rollback leaves them unused. A list belongs to exactly one account; a
-- todo belongs to exactly one list and is deleted with it.
--
-- There are deliberately no content checks in SQL: a failed CHECK makes
-- Postgres log the whole row, title included (design D9). The domain validates
-- names and titles. The open/done check only constrains state columns.
--
-- `position` orders open todos and is unused for done ones. The unique
-- constraint is deferrable so one statement can renumber a whole list without
-- transient collisions (design D6). `completed_at` orders the completed group,
-- newest first.

CREATE TABLE lists (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    creation_key UUID NOT NULL,
    name         TEXT NOT NULL,
    version      INTEGER NOT NULL DEFAULT 1,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (user_id, creation_key)
);

CREATE INDEX lists_user_created_idx ON lists (user_id, created_at, id);

CREATE TABLE todos (
    id           UUID PRIMARY KEY,
    list_id      UUID NOT NULL REFERENCES lists (id) ON DELETE CASCADE,
    creation_key UUID NOT NULL,
    title        TEXT NOT NULL,
    version      INTEGER NOT NULL DEFAULT 1,
    position     BIGINT,
    completed_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK ((position IS NULL) = (completed_at IS NOT NULL)),
    UNIQUE (list_id, creation_key),
    UNIQUE (list_id, position) DEFERRABLE INITIALLY IMMEDIATE
);

-- Open todos are read in position order; done ones never need that index.
CREATE INDEX todos_list_open_idx ON todos (list_id, position) WHERE position IS NOT NULL;
CREATE INDEX todos_list_done_idx ON todos (list_id, completed_at DESC) WHERE completed_at IS NOT NULL;
