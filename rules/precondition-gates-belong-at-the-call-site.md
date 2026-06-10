---
digest: A sum return whose only caller switches on it is a misplaced precondition gate — move the gate to the caller, make the function unconditional.
paths:
  - '**/*.rs'
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.ts'
  - '**/*.tsx'
blocking: false
---

**Precondition gates belong at the call site.** A function that returns
`enum { case ok(T); case blocked(Reason) }` and whose only caller switches on it
is bundling the work that produces `T` with a gate deciding whether to produce
it. The sum is intermediate data between produce and consume that nothing else
uses. Move the gate to the caller — the function becomes unconditional
construction; the caller checks the precondition before calling.

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

Doesn't fire for `Result<T, Error>` / `Optional<T>` (absence is domain) or sums
whose cases are all valid products. Diagnostic: one case carries the primary
output, others carry "didn't do it" *and* the caller has the inputs to evaluate
the gate itself.
