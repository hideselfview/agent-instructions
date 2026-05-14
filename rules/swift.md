---
paths:
  - '**/*.swift'
---

# Swift conventions

## Filename matches a real type inside

Every `.swift` file's name must reference something that exists in the file — a
primary type, an extension target, or a caseless-enum namespace. Filenames that
don't correspond to anything inside read as "where's the type?" and tend to
attract unrelated dumping over time.

Acceptable shapes:

1. **Single primary type** — `LightboxOverlay.swift` contains
   `struct LightboxOverlay`. Tightly-coupled secondary types (data types the
   primary consumes, sub-views) can coexist.
2. **Extension target** — `BridgeSortField+CaseIterable.swift` contains an
   extension on `BridgeSortField`. Apple's `<Type>+<Aspect>.swift` convention.
3. **Caseless-enum namespace** — `ImageLoader.swift` contains
   `enum ImageLoader { static func load... }`. Use this when grouping free
   functions or related types under a topic — public API inside the enum,
   private helpers at file scope.

When you find a violation:

- Single primary type with mismatched filename → rename file to match.
- Multiple unrelated types under a topic name → split into per-type files.
- Free functions in a topic-named file → wrap public API in
  `enum Topic { static func ... }`.
- Junk-drawer extensions file → split into per-type extension files.

## Layout stability in repeated views

Inside `ForEach`/`List` rows (or any repeated container), keep all possible
content in the layout tree at all times. Use `.opacity(0/1)` plus
`.allowsHitTesting(false/true)` to show/hide. Never
`if condition { SomeView() }` to conditionally include a child — that changes
the row's intrinsic size, dirties the parent stack, and forces recursive
re-measurement of every sibling row.

Symptom: a list of 100+ items with hover-revealing buttons drops to ~0.5 FPS on
hover. The hover flips `@State isHovered`, which conditionally adds a button to
a ZStack, which dirties the ZStack's intrinsic size, which dirties the VStack,
which re-lays-out all rows.

Outside repeated views (a single sheet, a one-off detail view), conditional
inclusion is fine.

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
state — see "State describes what is, not what should happen" in
`reactive-ui.md`. Use `@State Task` + handler instead.

**Cancellation safety lives in the async function**, not the view:
`Task.isCancelled` checks between stages, `withTaskCancellationHandler` for
synchronous cleanup, commit-at-end so cancel-before-commit means no state
changed.

**Fire-and-forget** (`Task { ... }` not stored) is the exception. Use only when
the work genuinely shouldn't be cancellable from the UI. Don't fire-and-forget
to skip designing cancellation safety — the user submitted the action; let them
abort.

## @Observable: mutate on the state, not the view

Computed properties on a SwiftUI view that read `@Observable` state, combined
with action handlers that read the same state and write back, cause runtime
exclusivity crashes. When SwiftUI dispatches an event during rendering (key
press, button tap), the read access from rendering is still open — the handler's
read+write triggers a simultaneous access violation.

Banned: view computed properties that read `@Observable` state + action handlers
that read those properties and write back to the same state. The read from
rendering and the write from the handler overlap.

Fix: mutation logic belongs on the state type, not the view.
`uiState.lightbox?.navigateNext()` is one access. Reading `currentIndex` then
writing `uiState.lightbox?.currentIndex` is two overlapping accesses.

Rule of thumb: if a view action mutates `@Observable` state based on its current
value, that read-modify-write must be a single `mutating func` on the state
type.

## Don't mask errors with `try?` and `??`

Concrete Swift instances of CLAUDE.md "Never mask errors with defaults."

`try?` rules:

- Banned: `(try? someCall()) ?? []` — returns empty results; user sees "no
  results" instead of "search failed." Use `do/catch` and set an error state.
- Banned: `(try? someCall()) ?? false` — hides failure behind a default boolean.
  Propagate the error.
- Banned: `try?` in a `Task.detached` with no error reporting — fire-and-forget
  that silently fails. Use `do/catch` and report via the project's error sink.
- Allowed: `try?` for genuinely non-critical operations where failure doesn't
  affect the user — temp file cleanup, `Task.sleep` cancellation, JSON
  serialization of non-essential caches.
- Allowed: `try?` inside a non-throwing closure where the result is cosmetic
  (e.g., a "likely-X" badge check).

`??` rules:

- Banned: `x ?? ""` to construct `URL(string: x ?? "")` — use
  `x.flatMap { URL(string: $0) }` instead. The empty string just produces nil
  with extra steps.
- Banned: `selectedThing?.value ?? 0` where `selectedThing` could be stale from
  an async callback. Capture the value at call time, not callback time.
- Allowed: `dict[key] ?? defaultValue` where the default is semantically correct
  (e.g., `progress[name] ?? 0` means 0% progress).
- Allowed: `optionalString ?? ""` for display-only text in SwiftUI `Text()`
  views — empty text is the correct visual for nil.
