-- UNIQUE (list_id, position) already has an index that serves every lookup this
-- one did, so it only cost a second write per add, mark and renumbering.
DROP INDEX todos_list_open_idx;
