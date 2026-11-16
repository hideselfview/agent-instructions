---
digest: >-
  A fast stream carries a value that changes many times a second, one latest
  value per key, as a channel would inside Rust: a value in Rust's memory
  (playback position, transfer bytes, rates, levels), or one that Rust computes
  from such a value and a live query's values, such as an upload bar's stored
  bytes plus the bytes its files being sent have moved. It never carries a
  plain query result, and core never writes its value to a table. In every bae
  app, that value reaches the screen through a native view or layer that reads
  the stream's latest value per key once per display frame, never through the
  UI framework's observed state (SwiftUI, Compose, Avalonia bindings) and never
  with a main-thread hop per change.
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.axaml'
blocking: true
---

## Native views draw fast-changing values

Some values change many times a second: playback position, the bytes a file
upload or download has moved, transfer rates, meter levels. Core sends them on
keyed streams, not in tables. In every bae app, a view that the stream sets
directly draws them, never the UI framework's observed state.

Observed state (SwiftUI `@Observable` or `@State`, Compose state read in
composition, an Avalonia view-model property behind a binding) reruns the
framework's update on every change: every view that reads the value evaluates,
compares and lays out again on the main thread. A fast upload sends hundreds of
changes a second across every visible row.

Each app splits the work in two: the framework places the view once and gives it
the stream's key, and the view reads the value itself. A new app follows the
same split from its first progress bar.

### What a fast stream carries

A fast stream carries one value per key, the latest one, the way a watch channel
carries it from one part of Rust to another. Each value changes many times a
second, and is one of two kinds:

- A value that lives in Rust's memory, such as the audio output's position or a
  transfer's byte count. Core sends it straight from where it has it to the view
  that draws it.
- A value that Rust computes from such a value together with a live query's
  values.
  - An upload's bar is the release's stored bytes, from the live query, plus,
    for each file being sent, its size times the share of it that coven's
    transfer watch says has moved. Its speed is measured from how fast those
    shares move.
  - A Rust watcher in the domain that owns the value computes it again each time
    the query reruns or the in-memory value changes
    (`reactive-screens-read-plain-live-queries.md`, "A fast value computed with
    a live query's values").
  - The view draws the value as the stream gives it, and adds nothing up.

Core never does either of these:

- Write a fast value to a table so a live query can read it. Each such write
  makes coven rerun screen queries many times a second to deliver one number.
- Put a plain query result, with no fast-changing part, on a fast stream. Such a
  value reaches the screen through the screen's live query, however often it
  changes. When that query reruns too often, the fix is in the query or in
  coven.

### Drawn once per frame, from the latest value per key

A screen shows at most one value per frame, and a transfer stream sends far more
than that: a fast upload sends about 1,700 changes a second per stream. So the
stream's callback never moves to the main thread with each change:

- The callback writes the latest value per key into a thread-safe store, on the
  thread the value arrives on.
  - Core's stream already merges the changes the reader hasn't taken.
- The native view that draws a key reads that key's latest value on the
  display's frame callback while the key is active.
  - It shows the value if it changed, and stops when the key goes idle.
  - The display's frame callback says when to draw; a timer never stands in for
    it.
- Only a key that starts or stops reaches the main thread, to start or stop the
  views that draw it.

bae's code for this is `LatestStream` and `FrameFollowing` in BaeKit (with an
`NSView` display link), and their Kotlin versions in `fm.bae.fastvalues` (with
`Choreographer`).

### macOS and iOS

The AppKit or UIKit view (or a `CALayer`) reads its key's latest value on each
frame of its display link (`NSView.displayLink`, `CADisplayLink`) and sets it: a
bar's fraction, a label's text, a ring's `strokeEnd`. SwiftUI places the view
once through a representable and passes it only the key's source. `updateNSView`
handles configuration (colors, the key the stream is for), never the value.
`TransferRingNSView`, `TransferReadoutNSView` and `StreamLabelNSView` follow
this pattern.

### Android

A Compose value read during composition recomposes on every change. Draw it in a
View (through `AndroidView`) that reads the key's latest value on a
`Choreographer` frame callback while the key is active, or in a draw lambda
(`Modifier.drawBehind`, `graphicsLayer`) that the frame callback invalidates.

### Windows and Linux

An Avalonia control reads the key's latest value on the compositor's frame
callback (`TopLevel.RequestAnimationFrame`) and never binds it through a view
model's `INotifyPropertyChanged`.

### Which values change fast

A fast value changes more than about once a second while it is active, and at
least one of the values it comes from lives in Rust's memory, not in a table.
Counts of stored rows are state from tables, however often a scan changes them,
and reach the screen through its live query like any other row data.

### Signs of a violation

- A store or view-model property that a progress or position stream updates,
  read in a SwiftUI `body`, a composable or a binding.
- A `Text("\(rate)")` fed from a throughput stream.
- A fast stream that carries a plain query result, with no fast-changing part,
  or a table that core writes several times a second so a screen's query can
  show progress.
- A stream callback that moves to the main thread (`Task { @MainActor in }`,
  `scope.launch(Dispatchers.Main)`) for each value of a fast stream.
