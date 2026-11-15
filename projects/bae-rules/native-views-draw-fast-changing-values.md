---
digest: In every bae app, a value that changes many times a second (playback position, transfer bytes, rates, levels) reaches the screen through a native view or layer that reads the stream's latest value per key once per display frame, never through the UI framework's observed state (SwiftUI, Compose, Avalonia bindings) and never with a main-thread hop per change.
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

A value changes fast when core sends it more than about once a second while it
is active. Counts that change when a state changes (queued, stored, pinned) are
state from tables, and go through the framework like any other row data.

### Signs of a violation

- A store or view-model property that a progress or position stream updates,
  read in a SwiftUI `body`, a composable or a binding.
- A `Text("\(rate)")` fed from a throughput stream.
- A stream callback that moves to the main thread (`Task { @MainActor in }`,
  `scope.launch(Dispatchers.Main)`) for each value of a fast stream.
