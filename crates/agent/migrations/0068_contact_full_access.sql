-- 200 CONTACT-FULL-ACCESS: a per-contact trust level on the mesh-wide contact record.
-- `contacts` rides the record replica (191), not the session log: SESSIONS_WIRE_SCHEMA stays 67.
-- NULL or 0 = off (fail closed), 1 = on. No constraint (record-replica rules, 0059).
ALTER TABLE contacts ADD COLUMN full_access INTEGER;
