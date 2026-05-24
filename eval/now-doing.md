# now-doing

Rules are atomic files — one per rule under `rules/<slug>.md` (and
`projects/<name>-rules/<slug>.md` for project rules), each carrying `digest` /
`paths` / `blocking` frontmatter. `generate.py` (run by `install.sh`) stitches
the digests into the always-on `instructions.md` index; Claude Code path-loads
each full body when a matching file is read. The CI reviewer is the reusable
matrix `agent-instructions/.github/workflows/rules-review.yml` — one rule per
job, reading its file directly from this repo. bae and forage each call it with
their own rule slug list (`rules-review-matrix.yml` / `ai-rules-review.yml`).

Still open: walk the release-identity PR chain (bae-fm/bae#636) — adjudicate
each PR's matrix findings together as true/false positives, fix the real ones,
then merge.
