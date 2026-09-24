-- Design doc 06 as-built (2026-09-24): the LLM model an external harness
-- (ACP child) reports for its session — the `model`-category select in
-- `session/new|load` `configOptions`, then `config_option_update`, then the
-- `agents.yaml` entry's `model:` — so a reloaded session shows `omp ·
-- anthropic/…` and not the harness name alone. NULL for native sessions and
-- for a harness that reports nothing.
ALTER TABLE sessions ADD COLUMN harness_model TEXT;
