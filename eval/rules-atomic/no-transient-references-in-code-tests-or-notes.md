**No transient references in code, tests, or notes.** Don't reference the
current task, fix, or session ("repro for today's bug", "fails on current code",
"the Downloads issue") in tests, comments, docstrings, or design notes. Describe
the timeless invariant instead. Transient context belongs in commit messages and
PR descriptions.
