-- 180 D1b: a ledger row that names a parent this store has no row for (a
-- child the record replica delivers before its parent, in pk order within a
-- batch or across origins) first gets a stand-in root for the parent, so the
-- child's INSERT/UPDATE satisfies `parent_id REFERENCES cost_owners(id)` and
-- lands instead of being skipped. The stand-in carries the sentinel HLC
-- (`accounting::STAND_IN_HLC`): the parent's real row overwrites it in place
-- (UPDATE, never delete-and-insert, so `remote_turns ON DELETE CASCADE` is
-- never triggered) when it arrives. Its own parent is filled by the UPDATE
-- trigger the same way. `INSERT OR IGNORE` only.
CREATE TRIGGER cost_owners_parent_stand_in_insert
BEFORE INSERT ON cost_owners
WHEN NEW.parent_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM cost_owners WHERE id = NEW.parent_id)
BEGIN
  INSERT OR IGNORE INTO cost_owners (id, root_id, parent_id, deleted_at, history_coverage, revision, hlc)
  VALUES (NEW.parent_id, NEW.root_id, NULL, NULL, 'unknown', 0, '0000000000000000-00000000-0');
END;

CREATE TRIGGER cost_owners_parent_stand_in_update
BEFORE UPDATE OF parent_id ON cost_owners
WHEN NEW.parent_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM cost_owners WHERE id = NEW.parent_id)
BEGIN
  INSERT OR IGNORE INTO cost_owners (id, root_id, parent_id, deleted_at, history_coverage, revision, hlc)
  VALUES (NEW.parent_id, NEW.root_id, NULL, NULL, 'unknown', 0, '0000000000000000-00000000-0');
END;
