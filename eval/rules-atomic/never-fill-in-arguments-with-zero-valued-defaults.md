**Never fill in arguments with zero-valued defaults.** Sibling to the rule
above, on the construction side. When you don't know what a parameter should be,
don't pass `0`/`nil`/`None`/`""` because it "looks safe" — trace to the real
value at the source. The default that type-checks often silently breaks
downstream (a `samples_to_skip: 0` looks harmless but causes seconds of replay
artifacts in audio playback). If the parameter genuinely is optional, the
signature should use `Option`/`Optional` so the absence is explicit. Exception:
test code can pass defaults for parameters not exercised by the test.
