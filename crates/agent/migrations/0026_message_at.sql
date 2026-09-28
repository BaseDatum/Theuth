-- Wall-clock time a message row was appended (RFC 3339 UTC), for the
-- transcript's hover timestamps and gap dividers (74 §5). NULL for rows
-- written before this migration.
ALTER TABLE messages ADD COLUMN at TEXT NULL;
