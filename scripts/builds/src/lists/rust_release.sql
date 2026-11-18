-- The queue item that builds made for each stable Rust release, so that one
-- release never makes a second item in the project (rust_release.rs owns this
-- table and is the only code that writes it; rust_stable.rs decides when).
-- The version is the key: a second item for a version is a refused write.

CREATE TABLE rust_release_item (
    version TEXT PRIMARY KEY CONSTRAINT rust_release_item_version_is_x_y_z CHECK (
        version GLOB '[0-9]*.[0-9]*.[0-9]*' AND version NOT GLOB '*[^0-9.]*' AND version NOT GLOB '*..*'
    ),
    item_id INTEGER NOT NULL UNIQUE REFERENCES item (id)
) STRICT, WITHOUT ROWID;
