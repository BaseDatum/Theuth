-- 160 H1 (review 160-ENG #1): a deleted owner keeps the `harness_model` its
-- session row had, so a remote meter priced by that fallback keeps its model
-- and provider in the cost dashboard after the session row is gone.
ALTER TABLE cost_owners ADD COLUMN harness_model TEXT NULL;
