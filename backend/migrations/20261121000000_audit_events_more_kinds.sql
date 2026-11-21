-- OT-008 slice 2: the remaining audited account actions.
--
-- Widens the kind list. Every existing row ('role_changed') passes the new
-- check, and the previous release writes only 'role_changed', so this is safe
-- mid-roll. Added NOT VALID (a brief lock, no scan); the next migration
-- validates existing rows without blocking writes (migrations/README.md).
ALTER TABLE audit_events DROP CONSTRAINT audit_events_kind_check;

ALTER TABLE audit_events
    ADD CONSTRAINT audit_events_kind_check
    CHECK (kind IN (
        'role_changed',
        'password_changed',
        'password_set',
        'password_reset',
        'sign_in_method_linked',
        'sign_in_method_unlinked',
        'deletion_requested',
        'deletion_cancelled',
        'account_deleted',
        'data_exported',
        'sessions_revoked_after_reuse'
    ))
    NOT VALID;
