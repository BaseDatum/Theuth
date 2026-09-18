-- Name of the opener (its `artifacts.owners/` lock file) that claimed the row;
-- NULL rows predate ownership and are recovered as `unknown` on next open.
ALTER TABLE app_invocations ADD COLUMN owner TEXT;
