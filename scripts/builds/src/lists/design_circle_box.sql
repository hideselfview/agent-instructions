-- A circle of a review board that surrounds a box on the board, not the
-- elements of a selector (design_circle_box.rs owns this table and is the only
-- code that writes it). `review_circle` holds the circles of a selector. A board's
-- circles read in the order of their positions across both tables, and a
-- position is in one of them.
CREATE TABLE review_circle_box (
    design_id INTEGER NOT NULL,
    board INTEGER NOT NULL,
    position INTEGER NOT NULL CONSTRAINT review_circle_box_position_not_negative CHECK (position >= 0),
    -- The box in board pixels: its left and top edge, its width and height.
    x INTEGER NOT NULL,
    y INTEGER NOT NULL,
    width INTEGER NOT NULL CONSTRAINT review_circle_box_width_is_positive CHECK (width > 0),
    height INTEGER NOT NULL CONSTRAINT review_circle_box_height_is_positive CHECK (height > 0),
    label TEXT CONSTRAINT review_circle_box_label_says_something CHECK (label IS NULL OR trim(label) <> ''),
    PRIMARY KEY (design_id, board, position),
    CONSTRAINT review_circle_box_is_on_a_board FOREIGN KEY (design_id, board) REFERENCES review_board (design_id, position) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
