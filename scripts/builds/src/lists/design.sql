-- Design changes: a change to the design files, recorded when its boards are
-- drawn. design.rs owns these tables and is the only code that writes them.
-- What landed is the change's closing and its commits, not stored here.

CREATE TABLE design (
    id INTEGER PRIMARY KEY,
    record_kind TEXT NOT NULL DEFAULT 'design' CONSTRAINT design_is_a_design CHECK (record_kind = 'design'),
    title TEXT NOT NULL CONSTRAINT design_title_is_one_line CHECK (
        trim(title) <> '' AND title = trim(title)
        AND instr(title, char(9)) = 0 AND instr(title, char(10)) = 0 AND instr(title, char(13)) = 0
    ),
    details TEXT NOT NULL CONSTRAINT design_details_say_something CHECK (trim(details) <> ''),
    repo TEXT NOT NULL REFERENCES repo (name),
    -- The worktree the boards are drawn in.
    worktree TEXT NOT NULL CONSTRAINT design_worktree_is_absolute CHECK (substr(worktree, 1, 1) = '/'),
    -- Why shared design files changed and no drawing did.
    no_app_place TEXT CONSTRAINT design_no_app_place_says_something CHECK (no_app_place IS NULL OR trim(no_app_place) <> ''),
    CONSTRAINT design_is_a_record FOREIGN KEY (id, record_kind) REFERENCES record (id, kind)
) STRICT;

-- The person's review; an amend deletes it.
CREATE TABLE design_approval (
    design_id INTEGER PRIMARY KEY REFERENCES design (id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    by TEXT NOT NULL CONSTRAINT design_approval_by_says_something CHECK (trim(by) <> '')
) STRICT;

-- Each design place the change touches, resolved by the item that does the
-- app work or by how it was checked that none is needed.
CREATE TABLE design_place (
    design_id INTEGER NOT NULL REFERENCES design (id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    item_id INTEGER REFERENCES item (id),
    matches TEXT CONSTRAINT design_place_matches_says_something CHECK (matches IS NULL OR trim(matches) <> ''),
    PRIMARY KEY (design_id, path),
    CONSTRAINT design_place_is_resolved_one_way CHECK ((item_id IS NULL) <> (matches IS NULL))
) STRICT, WITHOUT ROWID;
CREATE INDEX design_place_item ON design_place (item_id);

-- A board place's link on the published canvas.
CREATE TABLE design_board (
    design_id INTEGER NOT NULL,
    path TEXT NOT NULL CONSTRAINT design_board_path_is_a_board_file CHECK (path NOT GLOB '*#*' AND path NOT GLOB '*/'),
    link TEXT NOT NULL CONSTRAINT design_board_link_says_something CHECK (trim(link) <> ''),
    PRIMARY KEY (design_id, path),
    CONSTRAINT design_board_is_a_place FOREIGN KEY (design_id, path) REFERENCES design_place (design_id, path) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE review_section (
    design_id INTEGER NOT NULL REFERENCES design (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CONSTRAINT review_section_position_not_negative CHECK (position >= 0),
    heading TEXT NOT NULL CONSTRAINT review_section_heading_says_something CHECK (trim(heading) <> ''),
    words TEXT NOT NULL,
    PRIMARY KEY (design_id, position)
) STRICT, WITHOUT ROWID;

CREATE TABLE review_board (
    design_id INTEGER NOT NULL,
    section INTEGER NOT NULL,
    position INTEGER NOT NULL CONSTRAINT review_board_position_not_negative CHECK (position >= 0),
    board TEXT NOT NULL CONSTRAINT review_board_is_a_board_file CHECK (board NOT GLOB '*#*' AND board NOT GLOB '*/'),
    caption TEXT NOT NULL CONSTRAINT review_board_caption_says_something CHECK (trim(caption) <> ''),
    theme TEXT NOT NULL CONSTRAINT review_board_theme_known CHECK (theme IN ('light', 'dark')),
    PRIMARY KEY (design_id, section, position),
    CONSTRAINT review_board_is_on_a_section FOREIGN KEY (design_id, section) REFERENCES review_section (design_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
