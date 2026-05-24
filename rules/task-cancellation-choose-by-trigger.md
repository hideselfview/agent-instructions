---
digest: State-driven work → .task(id:); event-driven → @State Task + handler cancel; don't fake a flag for .task(id:).
paths:
  - '**/*.swift'
blocking: false
---

## Task cancellation: choose by trigger

*State-driven work* (the trigger IS a state change, the key IS real state):
`.task { ... }` or `.task(id: realState) { ... }`. SwiftUI manages
cancel-on-disappear and cancel-on-id-change. Examples: initial fetch on appear,
debounced search keyed on `searchText`, image load keyed on `cursor.current.id`.

*Event-driven work* (the trigger is a gesture, button, callback, lifecycle
event): store the Task in `@State`, cancel from handlers:

```swift
@State private var task: Task<Void, Never>?

func startMutation() {
    task?.cancel()
    task = Task {
        do { try await mutation() }
        catch is CancellationError { /* reset UI */ }
        catch { /* surface error */ }
    }
}
.onDisappear { task?.cancel() }
```

**Anti-pattern**: synthesizing a `Bool` flag (or composite string id) just so
`.task(id:)` has something to react to. That's a one-shot trigger dressed up as
state — see "State describes what is, not what should happen"
(`rules/state-describes-what-is-not-what-should-happen.md`). Use `@State Task` +
handler instead.

**Cancellation safety lives in the async function**, not the view:
`Task.isCancelled` checks between stages, `withTaskCancellationHandler` for
synchronous cleanup, commit-at-end so cancel-before-commit means no state
changed.

**Fire-and-forget** (`Task { ... }` not stored) is the exception. Use only when
the work genuinely shouldn't be cancellable from the UI. Don't fire-and-forget
to skip designing cancellation safety — the user submitted the action; let them
abort.
