ALTER TABLE sessions ADD COLUMN cut_at INTEGER NULL; -- undo marker: messages seq >= cut_at and epochs at_seq >= cut_at are hidden until the next write
