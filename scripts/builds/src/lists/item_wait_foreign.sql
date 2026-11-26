-- An item's wait on an item of another project: the project's name and the
-- item's id there. The other project's store holds that item, so nothing here
-- can point at it with a foreign key. builds reads the item's standing from
-- that store whenever it loads this one, and the wait is met when the item
-- lands or finishes, as a wait on an item of the same project is.
CREATE TABLE item_wait_foreign (
    item_id INTEGER NOT NULL REFERENCES item (id),
    project TEXT NOT NULL CONSTRAINT item_wait_foreign_names_a_project CHECK (trim(project) <> ''),
    on_item INTEGER NOT NULL CONSTRAINT item_wait_foreign_names_an_item CHECK (on_item > 0),
    PRIMARY KEY (item_id, project, on_item)
) STRICT, WITHOUT ROWID;
