-- Task accent colour: a palette name (`types::ACCENTS`), NULL = none. Display only
-- (graph and board cards). Replicated with the row; a peer without the column skips it.
ALTER TABLE tasks ADD COLUMN accent TEXT;
