**Use the project's logger for real logs.** Any log that will live in committed
code goes through the project's structured logger —
`tracing::info!`/`warn!`/`error!`, the project's `Logger.<category>` helper, the
language's standard logging crate, whatever the codebase uses. Not
`println!`/`print`/`console.log`. Structured loggers give you levels,
categories, filtering, persistence; stdout prints don't. Temporary investigative
prints are fine during active debugging but must be removed before commit.
