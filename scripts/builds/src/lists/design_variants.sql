-- The versions of one part of a board that an open design change's review
-- page shows side by side while the person and the design session compare
-- them (design_variants.rs owns these tables and is the only code that writes
-- them). The page draws the board once per set of props, at the commit kept
-- here, and crops each drawing to the first element the selector finds. The
-- variants live in the store, never in the repo, and landing or dropping the
-- change deletes them.
CREATE TABLE design_variants (
    design_id INTEGER PRIMARY KEY REFERENCES design (id) ON DELETE CASCADE,
    board TEXT NOT NULL CONSTRAINT design_variants_board_is_a_board_file CHECK (board GLOB '*.dc.html' AND board NOT GLOB '*#*' AND board NOT GLOB '*/'),
    selector TEXT NOT NULL CONSTRAINT design_variants_selector_says_something CHECK (trim(selector) <> ''),
    theme TEXT NOT NULL CONSTRAINT design_variants_theme_known CHECK (theme IN ('light', 'dark')),
    -- The commit the board is drawn at, in the change's worktree.
    tip TEXT NOT NULL CONSTRAINT design_variants_tip_is_a_commit CHECK (length(tip) = 40 AND tip NOT GLOB '*[^0-9a-f]*')
) STRICT;

-- One set of props the page draws the board with, as a JSON object: the
-- props its board's script reads from `this.props`.
CREATE TABLE design_variant (
    design_id INTEGER NOT NULL,
    position INTEGER NOT NULL CONSTRAINT design_variant_position_not_negative CHECK (position >= 0),
    props TEXT NOT NULL CONSTRAINT design_variant_props_are_an_object CHECK (CASE WHEN json_valid(props) THEN json_type(props) = 'object' ELSE 0 END),
    PRIMARY KEY (design_id, position),
    CONSTRAINT design_variant_is_of_the_variants FOREIGN KEY (design_id) REFERENCES design_variants (design_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
