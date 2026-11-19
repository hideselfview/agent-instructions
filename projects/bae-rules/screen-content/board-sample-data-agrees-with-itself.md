---
digest: "A bae board's sample data agrees with itself: a count matches the rows drawn (a \"12 tracks\" line has 12 rows), a length matches its tracks, and the same album, person or playlist has the same name, cover and facts on every board that shows it."
paths:
  - design/**
  - plans/**
blocking: true
---

## A board's sample data agrees with itself

Every number and name a board shows agrees with the rest of the board and with
the other boards of the same screen:

- A count matches what the board draws: a playlist whose header says "12 tracks"
  has 12 rows, and a list that says "3 folders" draws 3.
- A total matches its parts: a length is the sum of the lengths of the tracks
  drawn, and a size is the sum of the files' sizes.
- The same thing reads the same everywhere: an album, a person or a playlist
  keeps its name, cover, artist and facts on every board that shows it, and the
  person signed in is the same person on every board of a flow.

When a board can't draw every row, it scrolls or cuts the list at the pane's
edge; it never shows a count the rows contradict.
