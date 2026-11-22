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

## Precondition gates belong at the call site

A function that returns `enum { case ok(T); case blocked(Reason) }`, and whose
only caller switches on it, combines two things: the work that produces `T`, and
a check that decides whether to produce it. The enum is data passed between the
two that nothing else uses. Move the check to the caller: the function builds
its result unconditionally, and the caller checks the precondition before it
calls.

```swift
// Before
enum OpenResult { case ready(AppService); case needsUnlock(name: String) }
static func open(...) throws -> OpenResult {
    let handle = try initApp(...)
    if handle.getConfig().needsUnlock { return .needsUnlock(name: ...) }
    return .ready(AppService(handle: handle, ...))
}

// After
init(handle: AppHandle, ...) { ... }
// caller:
let handle = try initApp(...)
if handle.getConfig().needsUnlock { showUnlock(...); return }
self.service = AppService(handle: handle, ...)
```

The rule applies when one case carries the result, the others carry "didn't do
it", and the caller has the inputs to make the check itself. It doesn't apply to
`Result<T, Error>` or `Optional<T>`, where the absence is part of the domain, or
to an enum whose cases are all valid results.
