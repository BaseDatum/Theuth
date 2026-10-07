-- 126 §8.4: the training-grade accelerator an engine advertises
-- (`metal` | `cuda` | `cpu`, `mesh::Accelerator::as_str`); NULL from engines
-- older than this column. Replicates with the row; an older engine skips the
-- unknown column with a warning (`replica::apply`).
ALTER TABLE engine_stats ADD COLUMN accelerator TEXT;
