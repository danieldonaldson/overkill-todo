-- OT-008 retention: the hourly maintenance pass finds events past 7 years by
-- time. A plain CREATE INDEX (not CONCURRENTLY): the table is new and small.
CREATE INDEX audit_events_occurred_at_idx ON audit_events (occurred_at);
