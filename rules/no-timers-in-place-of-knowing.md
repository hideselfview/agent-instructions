---
digest: Never use a timer, sleep, delay, or poll to stand in for knowing something happened. Wait on the event, completion, or revision itself, or model the in-between as state (a flag, an enum value). Timers are only for time that is the requirement — rate pacing, backoff after a failure, a timeout that fails loud, debouncing input, playback clocks.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**No timers in place of knowing.** A timer, sleep, delay, or poll that stands in
for knowing something happened is banned: "sleep, then the state should be
there", "delay so A runs after B", "poll until it looks done". Wait on the
event, the completion, or the revision itself; where there is an in-between
moment, model it as state — a flag or an enum value both sides can read.

Timers stay only where time itself is the requirement: pacing to a rate limit,
backoff after a real failure, a timeout that fails loud, debouncing input,
playback clocks.
