# now-doing

Rules are atomic files — one per rule under `rules/<slug>.md` (and
`projects/<name>-rules/<slug>.md` for project rules), each carrying `digest` /
`paths` / `blocking` frontmatter. `generate.py` (run by `install.sh`) stitches
the digests into the always-on `instructions.md` index; Claude Code path-loads
each full body when a matching file is read. The per-rule review matrix is
deprecated; the agent holding the implementation context reviews its diff once
against the rules.

Still open: walk the release-identity PR chain (bae-fm/bae#636) — adjudicate
each PR's matrix findings together as true/false positives, fix the real ones,
then merge.
