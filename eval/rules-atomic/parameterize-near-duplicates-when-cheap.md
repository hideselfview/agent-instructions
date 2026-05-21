**Parameterize near-duplicates when cheap.** Two or more near-copy blocks
introduced in the same change should collapse into one function/component with
parameters when the parameterization is small, local, and the variation maps to
a real distinction (a literal, a different type, a small expression). Skip when
the abstraction would be awkward, invent a parameter without meaning, or couple
unrelated callers — but the default for obvious near-copies is to parameterize.
