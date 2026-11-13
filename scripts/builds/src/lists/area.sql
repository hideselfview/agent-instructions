-- The work queue's areas, in the order the sidebar shows them. area.rs owns
-- this table and is the only code that writes it; an item names its area
-- and a rename carries every item.

CREATE TABLE area (
    name TEXT PRIMARY KEY CONSTRAINT area_name_is_one_line CHECK (
        trim(name) <> '' AND name = trim(name)
        AND instr(name, char(9)) = 0 AND instr(name, char(10)) = 0 AND instr(name, char(13)) = 0
    ),
    position INTEGER NOT NULL CONSTRAINT area_position_not_negative CHECK (position >= 0)
        CONSTRAINT area_position_is_its_own UNIQUE
) STRICT;
