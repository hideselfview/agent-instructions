**Every bug fix starts with a failing test.** *Before* you debug, before you
even investigate — write a test that reproduces the failure. Run it, confirm it
fails. Then fix the code, run again, confirm it passes. No exceptions — even for
"obvious" fixes. The failing test is the receipt that you understood the bug,
not just patched a symptom; the passing test is the receipt that the fix
actually addressed it. When narrating a bug fix, don't say "The fix: …" before
there's a test — say "The test: …" first.
