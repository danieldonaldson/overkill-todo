-- Analytics retention (OT-005, design D8): delete visitor data older than 90
-- days from Umami's database, in one transaction. Run daily by the
-- `analytics-retention` service (deploy/analytics/retention.sh); checked
-- against a seeded copy of the real schema by
-- scripts/check-analytics-retention.sh. Re-run that check after any Umami
-- upgrade: table and column names are Umami 3.5.0's.
--
-- Umami declares its relations in Prisma only (relationMode = "prisma"), so
-- there are no foreign keys: nothing cascades, and sessions left without
-- events have to be removed here.
\set ON_ERROR_STOP on

BEGIN;

-- now() is fixed for the whole transaction, so every statement uses the same
-- cutoff.
CREATE TEMP TABLE retention_cutoff ON COMMIT DROP AS
SELECT now() - interval '90 days' AS at;

-- Events and everything recorded per event or per visit.
DELETE FROM event_data WHERE created_at < (SELECT at FROM retention_cutoff);
DELETE FROM website_event WHERE created_at < (SELECT at FROM retention_cutoff);
DELETE FROM revenue WHERE created_at < (SELECT at FROM retention_cutoff);
DELETE FROM error_event WHERE created_at < (SELECT at FROM retention_cutoff);
DELETE FROM heatmap_event WHERE created_at < (SELECT at FROM retention_cutoff);
DELETE FROM session_replay WHERE created_at < (SELECT at FROM retention_cutoff);
DELETE FROM session_replay_saved WHERE created_at < (SELECT at FROM retention_cutoff);

-- Sessions older than the cutoff with no event left. A session that started
-- earlier but has recent events stays.
DELETE FROM session s
WHERE s.created_at < (SELECT at FROM retention_cutoff)
  AND NOT EXISTS (SELECT 1 FROM website_event e WHERE e.session_id = s.session_id);

-- Per-session records: old ones, and any whose session is gone.
DELETE FROM session_data d
WHERE d.created_at < (SELECT at FROM retention_cutoff)
   OR NOT EXISTS (SELECT 1 FROM session s WHERE s.session_id = d.session_id);
DELETE FROM session_link l
WHERE l.created_at < (SELECT at FROM retention_cutoff)
   OR NOT EXISTS (SELECT 1 FROM session s WHERE s.session_id = l.session_id);

COMMIT;
