-- Design doc 75 §5: the context window size an external harness reports in
-- `usage_update.size`, so clients compute a percentage without a catalog
-- entry for the harness's model. Written once per distinct value. NULL for
-- native sessions and before the first report.
ALTER TABLE sessions ADD COLUMN context_size INTEGER;
