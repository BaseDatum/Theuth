-- 221 §7.3: a `secret_request` that stores into an external manager. JSON of the resolved
-- `DestinationView` (connection, names, status; never a value), replicated with the row so any
-- engine's card shows where the answer goes. NULL = the local vault only.
ALTER TABLE secret_requests ADD COLUMN destination TEXT;
