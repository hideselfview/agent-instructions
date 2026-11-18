---
digest: Never use a timer, sleep, delay or poll to stand in for knowing something happened. Wait on the event, completion or revision itself, or model the moment in between as state (a flag, an enum value). Timers are only for time that is the requirement - rate pacing, backoff after a failure, a timeout that fails loudly, debouncing input, playback clocks.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
blocking: false
---

## No timers in place of knowing

Never use a timer, sleep, delay or poll to stand in for knowing that something
happened: "sleep, then the state should be there", "delay so A runs after B",
"poll until it looks done". Wait on the event, the completion or the revision
itself. When there is a moment in between, model it as state, such as a flag or
an enum value both sides can read.

Use a timer only where time itself is the requirement: pacing to a rate limit,
backoff after a real failure, a timeout that fails loudly, debouncing input,
playback clocks.
