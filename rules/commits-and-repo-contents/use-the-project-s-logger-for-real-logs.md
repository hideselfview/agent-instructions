---
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
blocking: false
---

## Use the project's logger for real logs

A log in committed code goes through the project's structured logger: Rust's
`tracing` macros (`info!`, `warn!`, `error!`), the project's `Logger.<category>`
helper in Swift, or whatever logger the codebase uses. Never `println!`, `print`
or `console.log`.
