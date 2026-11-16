---
digest: >-
  A fast stream carries a value that lives in Rust's memory and changes many
  times a second (playback position, transfer bytes, rates, levels), one latest
  value per key, as a channel would inside Rust; it never carries a query's
  result, and core never writes its value to a table. In every bae app, that
  value reaches the screen through a native view or layer that reads the
  stream's latest value per key once per display frame, never through the UI
  framework's observed state (SwiftUI, Compose, Avalonia bindings) and never
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

### Fast streams carry Rust values, not query results

A fast stream carries values that live in Rust's memory: one value per key, the
latest one, the way a watch channel carries it from one part of Rust to another.
Core sends each value straight from where it has it, such as the audio output's
position or a transfer's byte count, to the view that draws it.

Live queries and fast streams never meet:

- Core never writes a fast value to a table so a live query can read it. That
  makes coven rerun screen queries many times a second to deliver one number.
- Core never puts a live query's result, or anything else read from SQL, on a
  fast stream. A value that SQL can read reaches the screen through the screen's
  live query, however often it changes. When that query reruns too often, the
  fix is in the query or in coven.

### A screen that shows a fast value beside its rows

Some screens show both: an upload's bar is the release's files already stored
(rows in tables) and the file being sent right now (a fast value). Core keeps
the two apart, and the view puts them together without arithmetic:

- The screen's live query returns the rows: the release, the fraction of it
  already stored, and each file being sent with its share of the release.
- The fast stream carries, under each file's key, only what changes many times a
  second: how far that file has gone, as a fraction, and its speed.
- The view draws the stored part from the row, and for each file being sent a
  segment as wide as its share, filled to the stream's fraction. It adds nothing
  up.

Observed state (SwiftUI `@Observable` or `@State`, Compose state read in
composition, an Avalonia view-model property behind a binding) reruns the
framework's update on every change: every view that reads the value evaluates,
compares and lays out again on the main thread. A fast upload sends hundreds of
changes a second across every visible row.

Each app splits the work in two: the framework places the view once and gives it
the stream's key, and the view reads the value itself. A new app follows the
same split from its first progress bar.

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

A fast value lives in Rust's memory, not in a table, and changes more than about
once a second while it is active. Counts of stored rows are state from tables,
however often a scan changes them, and reach the screen through its live query
like any other row data.

### Signs of a violation

- A store or view-model property that a progress or position stream updates,
  read in a SwiftUI `body`, a composable or a binding.
- A `Text("\(rate)")` fed from a throughput stream.
- A fast stream fed from a live query or a SQL read, or a table that core writes
  several times a second so a screen's query can show progress.
- A stream callback that moves to the main thread (`Task { @MainActor in }`,
  `scope.launch(Dispatchers.Main)`) for each value of a fast stream.
