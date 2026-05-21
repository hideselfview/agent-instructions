**Test the real unit, not a reconstruction.** The test must call the actual
function or service that has the bug. Manually reconstructing the conditions in
isolation (calling sub-functions in the order you think causes the bug) is just
another program — it proves nothing about the real code. Identify the unit that
contains the bug, write a test that exercises that unit.
