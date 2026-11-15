---
digest: In SwiftUI, work that a state change starts runs in .task(id:) keyed on that state; work that an event starts is a Task kept in @State and cancelled from handlers; never invent a flag only to give .task(id:) a key.
paths:
  - '**/*.swift'
blocking: false
---

## Choose a task's cancellation by what starts it

**Work a state change starts** runs in `.task { … }` or
`.task(id: realState) { … }`, keyed on real state. SwiftUI cancels it when the
view disappears or the key changes.

- Examples: the first fetch when a view appears, a debounced search keyed on
  `searchText`, an image load keyed on `cursor.current.id`.

**Work an event starts** (a gesture, a button, a callback, a lifecycle event) is
a `Task` kept in `@State` and cancelled from handlers:

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

Never make up a `Bool` flag, or a combined string ID, only to give `.task(id:)`
something to react to. Such a flag is a one-shot command stored as state
(`state-describes-what-is-not-what-should-happen`): use a `Task` in `@State` and
a handler instead.

The async function makes cancellation safe, not the view: it checks
`Task.isCancelled` between stages, uses `withTaskCancellationHandler` for
synchronous cleanup, and commits its result at the end, so a cancel before the
commit changes no state.

An unstored `Task { … }` that nothing can cancel is the exception. Use one only
when the user must not be able to cancel the work, never to avoid designing
cancellation: the user started the action, so let them stop it.
